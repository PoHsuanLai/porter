//! The Generic sign-in conversation. A mail account asks for an address and a password and
//! finds the servers from the address (the server too, when none is published); a DAV account
//! asks for the server, a user name and a password.

use super::login::SharedLogin;
use super::mail::{self, Looked};
use super::{dav, jmap};
use crate::io::{Io, SharedDns};
use crate::password::{parse_server, password, plain, secret_of, text_of};
use porter_core::sheet::{
    FieldAnswer, FieldKind, FieldSpec, Manual, Protocol, SignInFault, SignInInput, manual_form,
    parse_manual,
};
use porter_core::{
    AccountLabel, Claim, Credential, Family, LoginName, Restriction, SecretPurpose, SecretText,
    ServiceEndpoint,
};
use porter_provider::{ProviderSet, ProviderSpec, SignIn, SignInMode, SignInStep, Signed};

/// Which kind of generic account this is, from its provider file's discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Flavor {
    /// IMAP and SMTP, found from an address.
    Mail,
    /// CalDAV and CardDAV, found from a server.
    Dav,
    /// A provider that names its own servers (Fastmail, iCloud, Yahoo, GMX): the file's
    /// endpoints, asked for an address and a password only.
    Fixed,
}

/// What was typed that a later step still needs.
#[derive(Debug)]
struct Typed {
    login: String,
    password: SecretText,
}

#[derive(Debug)]
enum State {
    Fresh,
    /// The first form is on screen.
    Asked,
    /// The mail servers were not found; the server is asked for.
    AskedServer(Typed),
    /// Discovery is on screen, waiting for the person's confirmation.
    Reviewing(Box<Signed>),
    Ended,
}

/// The sign-in conversation of a generic account.
#[derive(Debug)]
pub struct GenericSignIn {
    io: Io,
    dns: SharedDns,
    providers: ProviderSet,
    login: SharedLogin,
    spec: ProviderSpec,
    flavor: Flavor,
    mode: SignInMode,
    state: State,
}

impl GenericSignIn {
    pub(super) fn new(
        io: Io,
        dns: SharedDns,
        providers: ProviderSet,
        login: SharedLogin,
        spec: ProviderSpec,
        flavor: Flavor,
        mode: SignInMode,
    ) -> Self {
        Self {
            io,
            dns,
            providers,
            login,
            spec,
            flavor,
            mode,
            state: State::Fresh,
        }
    }

    fn form(flavor: Flavor) -> Vec<FieldSpec> {
        match flavor {
            Flavor::Mail | Flavor::Fixed => vec![plain(FieldKind::Address), password()],
            Flavor::Dav => vec![
                plain(FieldKind::Server),
                plain(FieldKind::Username),
                password(),
            ],
        }
    }

    fn failed(&mut self, fault: SignInFault) -> SignInStep {
        self.state = State::Ended;
        SignInStep::Failed(fault)
    }

    /// The account is known: the password is tried at its mail server (a wrong one, or a server
    /// that does not answer, ends the sign-in here), then it is reviewed (a new account) or
    /// finished (a sign-in again).
    async fn found(
        &mut self,
        credential: Credential,
        label: String,
        endpoints: Vec<ServiceEndpoint>,
        claims: Vec<Claim>,
    ) -> SignInStep {
        if let Err(fault) = self.login.check(&credential, &endpoints).await {
            return self.failed(fault);
        }
        let signed = Signed {
            label: AccountLabel(label),
            credentials: vec![(SecretPurpose::Password, credential)],
            claims,
            endpoints,
            restriction: Restriction::none(),
        };
        match self.mode {
            SignInMode::Add => {
                let step = SignInStep::Review {
                    claims: signed.claims.clone(),
                    endpoints: signed.endpoints.clone(),
                    restriction: signed.restriction.clone(),
                    label: signed.label.clone(),
                };
                self.state = State::Reviewing(Box::new(signed));
                step
            }
            SignInMode::Reauthenticate { .. } => {
                self.state = State::Ended;
                SignInStep::Done(signed)
            }
        }
    }

    async fn submitted(&mut self, answers: Vec<FieldAnswer>) -> SignInStep {
        match self.flavor {
            Flavor::Mail => self.submitted_mail(answers).await,
            Flavor::Dav => self.submitted_dav(answers).await,
            Flavor::Fixed => self.submitted_fixed(answers).await,
        }
    }

    async fn submitted_fixed(&mut self, answers: Vec<FieldAnswer>) -> SignInStep {
        let (Some(address), Some(password)) = (
            text_of(&answers, FieldKind::Address),
            secret_of(&answers, FieldKind::Password),
        ) else {
            return self.failed(SignInFault::Unreadable);
        };
        if mail::domain_of(&address).is_none() {
            return self.failed(SignInFault::Unreadable);
        }
        match mail::fixed(&address, &self.spec) {
            Ok((endpoints, claims)) => {
                self.found(Credential::Password(password), address, endpoints, claims)
                    .await
            }
            Err(fault) => self.failed(fault),
        }
    }

    async fn submitted_mail(&mut self, answers: Vec<FieldAnswer>) -> SignInStep {
        let (Some(address), Some(password)) = (
            text_of(&answers, FieldKind::Address),
            secret_of(&answers, FieldKind::Password),
        ) else {
            return self.failed(SignInFault::Unreadable);
        };
        if mail::domain_of(&address).is_none() {
            return self.failed(SignInFault::Unreadable);
        }
        match mail::look(&self.io, &self.dns, &self.providers, &address).await {
            Looked::Found(endpoints, claims) => {
                let label = address;
                self.found(Credential::Password(password), label, endpoints, claims)
                    .await
            }
            Looked::Ask => {
                let domain = mail::domain_of(&address).map(|d| d.as_str().to_owned());
                self.state = State::AskedServer(Typed {
                    login: address,
                    password,
                });
                SignInStep::AskFields(manual_form(Protocol::Imap, domain.as_deref()))
            }
            Looked::Offline => self.failed(SignInFault::Unreachable),
        }
    }

    async fn submitted_dav(&mut self, answers: Vec<FieldAnswer>) -> SignInStep {
        let (Some(server), Some(user), Some(password)) = (
            text_of(&answers, FieldKind::Server).and_then(|t| parse_server(&t)),
            text_of(&answers, FieldKind::Username),
            secret_of(&answers, FieldKind::Password),
        ) else {
            return self.failed(SignInFault::Unreadable);
        };
        let login = LoginName(user.clone());
        match dav::discover(&self.io, &self.spec, &server, &login, &password).await {
            Ok(found) => {
                let label = format!("{user}@{}", server.origin().host);
                self.found(
                    Credential::Password(password),
                    label,
                    found.endpoints,
                    found.claims,
                )
                .await
            }
            Err(fault) => self.failed(fault),
        }
    }

    /// The servers the person typed. The sheet's machine has checked them already; a form that
    /// still does not read is unreadable. The endpoints go the way a looked-up candidate's do:
    /// to the review, then stored. A JMAP API token, when typed, is the credential instead of
    /// the password.
    async fn typed_server(&mut self, typed: Typed, answers: &[FieldAnswer]) -> SignInStep {
        let Ok(manual) = parse_manual(answers) else {
            return self.failed(SignInFault::Unreadable);
        };
        let login = LoginName(manual.login().map_or(typed.login.clone(), str::to_owned));
        let mut credential = Credential::Password(typed.password.clone());
        let built = match &manual {
            Manual::Imap(servers) => mail::typed(Family::Imap, servers, &typed.login, &self.spec),
            Manual::Pop3(servers) => mail::typed(Family::Pop3, servers, &typed.login, &self.spec),
            Manual::Jmap(server) => {
                if let Some(token) = &server.token {
                    credential = Credential::Bearer(token.clone());
                }
                jmap::session(&self.io, &server.session, &login, &credential).await
            }
        };
        match built {
            Ok((endpoints, claims)) => {
                let label = typed.login.clone();
                self.found(credential, label, endpoints, claims).await
            }
            Err(fault) => self.failed(fault),
        }
    }
}

impl SignIn for GenericSignIn {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let state = std::mem::replace(&mut self.state, State::Ended);
        match (state, input) {
            (_, SignInInput::Cancel) => SignInStep::Failed(SignInFault::Cancelled),
            // Begin, and begin again after a step back.
            (_, SignInInput::Start) => {
                self.state = State::Asked;
                SignInStep::AskFields(Self::form(self.flavor))
            }
            (State::Asked, SignInInput::Fields(answers)) => self.submitted(answers).await,
            (State::AskedServer(typed), SignInInput::Fields(answers)) => {
                self.typed_server(typed, &answers).await
            }
            (State::Reviewing(signed), SignInInput::Confirm(_)) => SignInStep::Done(*signed),
            _ => self.failed(SignInFault::Unreadable),
        }
    }
}
