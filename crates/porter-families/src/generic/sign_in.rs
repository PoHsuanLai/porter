//! The Generic sign-in flow. A mail account asks for an address and a password and
//! finds the servers from the address (the server too, when none is published); a DAV account
//! asks for the server, a user name and a password.

use super::login::SharedLogin;
use super::mail::{self, Looked};
use super::{dav, jmap};
use crate::conversation::{Flow, Move};
use crate::io::{Io, SharedDns};
use crate::password::{parse_server, password_for, plain, text_of, typed_password};
use porter_capability::Claim;
use porter_core::{
    AccountLabel, Credential, Family, LoginName, Restriction, SecretPurpose, SecretText,
    ServiceEndpoint,
};
use porter_provider::{ProviderSet, ProviderSpec, SignInMode, SignInStep, Signed};
use porter_sheet::{
    FieldAnswer, FieldKind, FieldSpec, Manual, Protocol, SignInFault, SignInInput, manual_form,
    parse_manual,
};

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
pub struct Typed {
    login: String,
    password: SecretText,
}

/// Where the conversation waits.
#[derive(Debug)]
pub enum Mid {
    /// The first form is on screen.
    Asked,
    /// The mail servers were not found; the server is asked for.
    AskedServer(Typed),
}

/// Whether `found` are at the hosts the account `held` already (each found host is one of
/// them), so a sign-in again may use them.
fn same_hosts(found: &[ServiceEndpoint], held: &[ServiceEndpoint]) -> bool {
    !held.is_empty()
        && found.iter().all(|f| {
            held.iter().any(|h| {
                h.url
                    .origin()
                    .host
                    .eq_ignore_ascii_case(&f.url.origin().host)
            })
        })
}

/// The sign-in flow of a generic account.
#[derive(Debug)]
pub struct GenericFlow {
    io: Io,
    dns: SharedDns,
    providers: ProviderSet,
    login: SharedLogin,
    spec: ProviderSpec,
    flavor: Flavor,
    mode: SignInMode,
    /// Whether the review on screen is of servers only unsigned DNS named, whose password is
    /// tried once the person confirms.
    untried: bool,
}

impl GenericFlow {
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
            untried: false,
        }
    }

    /// The first form: the password field is an app password for a provider that takes only one
    /// (ship-5).
    fn form(&self) -> Vec<FieldSpec> {
        match self.flavor {
            Flavor::Mail | Flavor::Fixed => {
                vec![plain(FieldKind::Address), password_for(&self.spec)]
            }
            Flavor::Dav => vec![
                plain(FieldKind::Server),
                plain(FieldKind::Username),
                password_for(&self.spec),
            ],
        }
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
    ) -> Move<Mid> {
        if let Err(fault) = self.login.check(&credential, &endpoints).await {
            return Move::Fail(fault);
        }
        let signed = Self::signed(credential, label, endpoints, claims);
        match self.mode {
            SignInMode::Add => self.review(signed, false),
            SignInMode::Reauthenticate { .. } => Move::Done(signed),
            // a variant a newer porter adds: the sign-in ends as unreadable
            _ => Move::Fail(SignInFault::Unreadable),
        }
    }

    /// Servers that only unsigned DNS names, outside the address's domain: the password goes
    /// nowhere until the person has seen them. A new account is reviewed first and the password
    /// tried on `Confirm`. Signing in again shows no review (the sheet confirms it unseen), so
    /// the servers are used only when they are the ones the account already has, which the
    /// person confirmed when it was added; otherwise the person is asked for the server.
    async fn found_unconfirmed(
        &mut self,
        typed: Typed,
        endpoints: Vec<ServiceEndpoint>,
        claims: Vec<Claim>,
    ) -> Move<Mid> {
        let credential = Credential::Password(typed.password.clone());
        match &self.mode {
            SignInMode::Add => {
                let signed = Self::signed(credential, typed.login, endpoints, claims);
                self.review(signed, true)
            }
            SignInMode::Reauthenticate {
                endpoints: held, ..
            } if same_hosts(&endpoints, held) => {
                self.found(credential, typed.login, endpoints, claims).await
            }
            SignInMode::Reauthenticate { .. } => self.ask_server(typed),
            // a variant a newer porter adds: the sign-in ends as unreadable
            _ => Move::Fail(SignInFault::Unreadable),
        }
    }

    fn signed(
        credential: Credential,
        label: String,
        endpoints: Vec<ServiceEndpoint>,
        claims: Vec<Claim>,
    ) -> Signed {
        Signed::new(
            AccountLabel(label),
            vec![(SecretPurpose::Password, credential)],
            claims,
            endpoints,
            Restriction::none(),
        )
    }

    /// Shows `signed` for review and waits for the person; `untried` says whether its password
    /// is still to be tried at the servers.
    fn review(&mut self, signed: Signed, untried: bool) -> Move<Mid> {
        self.untried = untried;
        Move::Review(signed)
    }

    /// Asks for the mail server, keeping what was typed.
    fn ask_server(&self, typed: Typed) -> Move<Mid> {
        let domain = mail::domain_of(&typed.login).map(|d| d.as_str().to_owned());
        Move::Next(
            SignInStep::AskFields(manual_form(Protocol::Imap, domain.as_deref())),
            Mid::AskedServer(typed),
        )
    }

    async fn submitted(&mut self, answers: Vec<FieldAnswer>) -> Move<Mid> {
        match self.flavor {
            Flavor::Mail => self.submitted_mail(answers).await,
            Flavor::Dav => self.submitted_dav(answers).await,
            Flavor::Fixed => self.submitted_fixed(answers).await,
        }
    }

    async fn submitted_fixed(&mut self, answers: Vec<FieldAnswer>) -> Move<Mid> {
        let (Some(address), Some(password)) = (
            text_of(&answers, FieldKind::Address),
            typed_password(&answers),
        ) else {
            return Move::Fail(SignInFault::Unreadable);
        };
        if mail::domain_of(&address).is_none() {
            return Move::Fail(SignInFault::Unreadable);
        }
        match mail::fixed(&address, &self.spec) {
            Ok((endpoints, claims)) => {
                self.found(Credential::Password(password), address, endpoints, claims)
                    .await
            }
            Err(fault) => Move::Fail(fault),
        }
    }

    async fn submitted_mail(&mut self, answers: Vec<FieldAnswer>) -> Move<Mid> {
        let (Some(address), Some(password)) = (
            text_of(&answers, FieldKind::Address),
            typed_password(&answers),
        ) else {
            return Move::Fail(SignInFault::Unreadable);
        };
        if mail::domain_of(&address).is_none() {
            return Move::Fail(SignInFault::Unreadable);
        }
        match mail::look(&self.io, &self.dns, &self.providers, &address).await {
            Looked::Found {
                endpoints,
                claims,
                confirm_first: false,
            } => {
                let label = address;
                self.found(Credential::Password(password), label, endpoints, claims)
                    .await
            }
            Looked::Found {
                endpoints,
                claims,
                confirm_first: true,
            } => {
                let typed = Typed {
                    login: address,
                    password,
                };
                self.found_unconfirmed(typed, endpoints, claims).await
            }
            Looked::Ask => self.ask_server(Typed {
                login: address,
                password,
            }),
            Looked::Offline => Move::Fail(SignInFault::Unreachable),
        }
    }

    async fn submitted_dav(&mut self, answers: Vec<FieldAnswer>) -> Move<Mid> {
        let (Some(server), Some(user), Some(password)) = (
            text_of(&answers, FieldKind::Server).and_then(|t| parse_server(&t)),
            text_of(&answers, FieldKind::Username),
            typed_password(&answers),
        ) else {
            return Move::Fail(SignInFault::Unreadable);
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
            Err(fault) => Move::Fail(fault),
        }
    }

    /// The servers the person typed. The sheet's machine has checked them already; a form that
    /// still does not read is unreadable. The endpoints go the way a looked-up candidate's do:
    /// to the review, then stored. A JMAP API token, when typed, is the credential instead of
    /// the password.
    async fn typed_server(&mut self, typed: Typed, answers: &[FieldAnswer]) -> Move<Mid> {
        let Ok(manual) = parse_manual(answers) else {
            return Move::Fail(SignInFault::Unreadable);
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
            Err(fault) => Move::Fail(fault),
        }
    }
}

impl Flow for GenericFlow {
    type Mid = Mid;

    async fn start(&mut self) -> Move<Mid> {
        self.untried = false;
        Move::Next(SignInStep::AskFields(self.form()), Mid::Asked)
    }

    async fn resume(&mut self, mid: Mid, input: SignInInput) -> Move<Mid> {
        match (mid, input) {
            (Mid::Asked, SignInInput::Fields(answers)) => self.submitted(answers).await,
            (Mid::AskedServer(typed), SignInInput::Fields(answers)) => {
                self.typed_server(typed, &answers).await
            }
            _ => Move::Fail(SignInFault::Unreadable),
        }
    }

    /// The person confirmed servers whose password was not tried yet: it is tried now, and a
    /// wrong one, or a server that does not answer, ends the sign-in.
    async fn confirm(&mut self, signed: Signed) -> Result<Signed, SignInFault> {
        if !std::mem::take(&mut self.untried) {
            return Ok(signed);
        }
        match signed.credentials.first() {
            Some((_, credential)) => self.login.check(credential, &signed.endpoints).await?,
            None => return Err(SignInFault::Unreadable),
        }
        Ok(signed)
    }
}
