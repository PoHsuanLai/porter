//! The three ways the servers of a generic account are found: from the address (`MailLookup`),
//! from the server's name (`DavLookup`), and from the provider's own file (`FixedLookup`).

use super::mail::{self, Looked};
use super::sign_in::{Located, Lookup, Outcome, Seams};
use super::{dav, jmap};
use crate::password::{parse_server, password_for, plain, text_of, typed_password};
use porter_capability::Claim;
use porter_core::{Credential, Family, LoginName, SecretText, ServiceEndpoint};
use porter_provider::{ProviderSpec, SignInMode, SignInStep};
use porter_sheet::{
    FieldAnswer, FieldKind, FieldSpec, Manual, Protocol, SignInFault, manual_form, parse_manual,
};

/// IMAP and SMTP, found from an address: autoconfig, SRV and MX, and the server the person types
/// when none is published.
#[derive(Debug, Clone, Copy, Default)]
pub struct MailLookup;

/// CalDAV and CardDAV, found from a server's name.
#[derive(Debug, Clone, Copy, Default)]
pub struct DavLookup;

/// A provider that names its own servers (Fastmail, iCloud, Yahoo, GMX): the file's endpoints,
/// asked for an address and a password only.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixedLookup;

/// What was typed that the question for the mail server still needs.
#[derive(Debug)]
pub struct Typed {
    login: String,
    password: SecretText,
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

/// An address and a password: the first form of the lookups that start from an address.
fn address_form(spec: &ProviderSpec) -> Vec<FieldSpec> {
    vec![plain(FieldKind::Address), password_for(spec)]
}

/// The address and the password typed, if both were.
fn address_and_password(answers: &[FieldAnswer]) -> Option<(String, SecretText)> {
    let (address, password) = (
        text_of(answers, FieldKind::Address)?,
        typed_password(answers)?,
    );
    mail::domain_of(&address).map(|_| (address, password))
}

impl MailLookup {
    /// Servers that only unsigned DNS names, outside the address's domain: the password goes
    /// nowhere until the person has seen them. A new account is reviewed first and the password
    /// tried on `Confirm`. Signing in again shows no review (the sheet confirms it unseen), so
    /// the servers are used only when they are the ones the account already has, which the
    /// person confirmed when it was added; otherwise the person is asked for the server.
    fn unconfirmed(
        seams: &Seams<'_>,
        typed: Typed,
        endpoints: Vec<ServiceEndpoint>,
        claims: Vec<Claim>,
    ) -> Outcome<Typed> {
        match seams.mode {
            SignInMode::Add => Outcome::Unseen(
                Located {
                    credential: Credential::Password(typed.password),
                    label: typed.login,
                    endpoints,
                    claims,
                }
                .into_signed(),
            ),
            SignInMode::Reauthenticate {
                endpoints: held, ..
            } if same_hosts(&endpoints, held) => Outcome::Known(Located {
                credential: Credential::Password(typed.password),
                label: typed.login,
                endpoints,
                claims,
            }),
            SignInMode::Reauthenticate { .. } => Self::ask_server(typed),
            // a variant a newer porter adds: the sign-in ends as unreadable
            _ => Outcome::Fail(SignInFault::Unreadable),
        }
    }

    /// Asks for the mail server, keeping what was typed.
    fn ask_server(typed: Typed) -> Outcome<Typed> {
        let domain = mail::domain_of(&typed.login).map(|d| d.as_str().to_owned());
        Outcome::Ask(
            SignInStep::AskFields(manual_form(Protocol::Imap, domain.as_deref())),
            typed,
        )
    }
}

impl Lookup for MailLookup {
    type Mid = Typed;

    fn form(spec: &ProviderSpec) -> Vec<FieldSpec> {
        address_form(spec)
    }

    async fn submitted(&self, seams: &Seams<'_>, answers: Vec<FieldAnswer>) -> Outcome<Typed> {
        let Some((address, password)) = address_and_password(&answers) else {
            return Outcome::Fail(SignInFault::Unreadable);
        };
        match mail::look(seams.io, seams.dns, seams.providers, &address).await {
            Looked::Found {
                endpoints,
                claims,
                confirm_first: false,
            } => Outcome::Known(Located {
                credential: Credential::Password(password),
                label: address,
                endpoints,
                claims,
            }),
            Looked::Found {
                endpoints,
                claims,
                confirm_first: true,
            } => {
                let typed = Typed {
                    login: address,
                    password,
                };
                Self::unconfirmed(seams, typed, endpoints, claims)
            }
            Looked::Ask => Self::ask_server(Typed {
                login: address,
                password,
            }),
            Looked::Offline => Outcome::Fail(SignInFault::Unreachable),
        }
    }

    /// The servers the person typed. The sheet's machine has checked them already; a form that
    /// still does not read is unreadable. The endpoints go the way a looked-up candidate's do:
    /// to the review, then stored. A JMAP API token, when typed, is the credential instead of
    /// the password.
    async fn resumed(
        &self,
        seams: &Seams<'_>,
        typed: Typed,
        answers: Vec<FieldAnswer>,
    ) -> Outcome<Typed> {
        let Ok(manual) = parse_manual(&answers) else {
            return Outcome::Fail(SignInFault::Unreadable);
        };
        let login = LoginName(manual.login().map_or(typed.login.clone(), str::to_owned));
        let mut credential = Credential::Password(typed.password.clone());
        let built = match &manual {
            Manual::Imap(servers) => mail::typed(Family::Imap, servers, &typed.login, seams.spec),
            Manual::Pop3(servers) => mail::typed(Family::Pop3, servers, &typed.login, seams.spec),
            Manual::Jmap(server) => {
                if let Some(token) = &server.token {
                    credential = Credential::Bearer(token.clone());
                }
                jmap::session(seams.io, &server.session, &login, &credential).await
            }
        };
        match built {
            Ok((endpoints, claims)) => Outcome::Known(Located {
                credential,
                label: typed.login,
                endpoints,
                claims,
            }),
            Err(fault) => Outcome::Fail(fault),
        }
    }
}

impl Lookup for DavLookup {
    type Mid = ();

    fn form(spec: &ProviderSpec) -> Vec<FieldSpec> {
        vec![
            plain(FieldKind::Server),
            plain(FieldKind::Username),
            password_for(spec),
        ]
    }

    async fn submitted(&self, seams: &Seams<'_>, answers: Vec<FieldAnswer>) -> Outcome<()> {
        let (Some(server), Some(user), Some(password)) = (
            text_of(&answers, FieldKind::Server).and_then(|t| parse_server(&t)),
            text_of(&answers, FieldKind::Username),
            typed_password(&answers),
        ) else {
            return Outcome::Fail(SignInFault::Unreadable);
        };
        let login = LoginName(user.clone());
        match dav::discover(seams.io, seams.spec, &server, &login, &password).await {
            Ok(found) => Outcome::Known(Located {
                credential: Credential::Password(password),
                label: format!("{user}@{}", server.origin().host),
                endpoints: found.endpoints,
                claims: found.claims,
            }),
            Err(fault) => Outcome::Fail(fault),
        }
    }
}

impl Lookup for FixedLookup {
    type Mid = ();

    fn form(spec: &ProviderSpec) -> Vec<FieldSpec> {
        address_form(spec)
    }

    async fn submitted(&self, seams: &Seams<'_>, answers: Vec<FieldAnswer>) -> Outcome<()> {
        let Some((address, password)) = address_and_password(&answers) else {
            return Outcome::Fail(SignInFault::Unreadable);
        };
        match mail::fixed(&address, seams.spec) {
            Ok((endpoints, claims)) => Outcome::Known(Located {
                credential: Credential::Password(password),
                label: address,
                endpoints,
                claims,
            }),
            Err(fault) => Outcome::Fail(fault),
        }
    }
}
