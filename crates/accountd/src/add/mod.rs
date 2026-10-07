//! `accountd add`: add an account from a terminal, for a computer with no sheet host yet.
//!
//! It runs the same `AccountService::add_account` the sheet host drives, with a terminal in the
//! sheet's place (`terminal`): a pasted key is read with echo off (a browser page, for a provider
//! that has one, is printed and opened), and the secret is filed through the same `Secrets` and registry the daemon uses. A
//! key or a password goes into the sign-in and nowhere else: not onto a bus, not into a log.
//!
//! **Consent.** Each `--allow <app>` records the grant an `Allow, always` answer to that app's
//! consent sheet would: the person typing the command is the consent. The grant is for the new
//! account, for the Llm kind, interactive use, in every data class (or the `--class` ones), and it
//! is the whole of what the app is given: inferd still lowers nothing, and `ai.floor.<class>` still
//! decides which classes may leave this computer.
//!
//! **The daemon.** accountd reads its registry when it starts and writes it itself; two writers
//! would lose an update. So the command takes the bus name `org.quire.Accounts1` for as long as
//! it runs, which is also the lock: a running daemon already owns the name and the command stops
//! with `AddError::DaemonRunning` (stop the unit, run the command, start the unit), and a daemon
//! that starts meanwhile cannot take the name, so it never reads half a registry. When there is no
//! session bus the command runs without the lock and says so.

mod terminal;

pub use terminal::{Echo, StdTerminal, Terminal, TerminalLink, TerminalSheets};

use crate::core::slug;
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    AccountId, AccountsReply, AccountsRequest, AppId, AppName, DataClass, GrantId, Isolation, Need,
    ProviderId, Tokens,
};
use porter_dbus::ACCOUNTS_BUS;
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, AuditSink, Clock, RegistryStore};
use std::collections::BTreeSet;
use zbus::fdo::RequestNameReply;

/// Every data class, for `--allow` without `--class`.
pub const ALL_CLASSES: [DataClass; 12] = [
    DataClass::AppOwn,
    DataClass::Mail,
    DataClass::Calendar,
    DataClass::Contacts,
    DataClass::Notes,
    DataClass::Files,
    DataClass::Photos,
    DataClass::Clipboard,
    DataClass::Screen,
    DataClass::Voice,
    DataClass::Prompt,
    DataClass::Public,
];

/// What the person asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddArgs {
    /// The provider file's id (`anthropic`, `openai`).
    pub provider: ProviderId,
    /// The apps to give the account to.
    pub allow: Vec<AppId>,
    /// The data classes to grant; every class when empty.
    pub classes: Vec<DataClass>,
}

impl AddArgs {
    /// The arguments as typed: a provider id, `--allow` apps as `name` or `name:isolation`
    /// (`org.quire.Companion`, `org.example.App:flatpak`; unsandboxed when none is given, the
    /// isolation of a native app started from a systemd unit), and `--class` slugs.
    pub fn parse(provider: &str, allow: &[String], classes: &[String]) -> Result<Self, AddError> {
        let bad = |what: &str, text: &str| AddError::Argument(format!("{what} `{text}`"));
        let provider = ProviderId::parse(provider).map_err(|_| bad("provider id", provider))?;
        let allow = allow
            .iter()
            .map(|text| {
                let (name, isolation) = match text.split_once(':') {
                    Some((name, isolation)) => (name, slug::<Isolation>(isolation).ok()),
                    None => (text.as_str(), Some(Isolation::Unsandboxed)),
                };
                Ok(AppId {
                    name: AppName::parse(name).map_err(|_| bad("app id", text))?,
                    isolation: isolation.ok_or_else(|| bad("isolation in", text))?,
                })
            })
            .collect::<Result<Vec<_>, AddError>>()?;
        let classes = classes
            .iter()
            .map(|text| slug::<DataClass>(text).map_err(|_| bad("data class", text)))
            .collect::<Result<Vec<_>, AddError>>()?;
        Ok(Self {
            provider,
            allow,
            classes,
        })
    }

    fn granted_classes(&self) -> Vec<DataClass> {
        match self.classes.is_empty() {
            true => ALL_CLASSES.to_vec(),
            false => self
                .classes
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }
}

/// What was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddReport {
    /// The new account.
    pub account: AccountId,
    /// Each grant made: the app, the class and the grant's id.
    pub grants: Vec<(AppId, DataClass, GrantId)>,
}

/// Why `accountd add` did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    /// An argument is not what it says.
    Argument(String),
    /// accountd owns `org.quire.Accounts1`.
    DaemonRunning,
    /// No provider file of that id is served.
    UnknownProvider(ProviderId),
    /// The account was added, and an `--allow` grant after it was not.
    AllowFailed {
        /// The account that exists now.
        account: AccountId,
        /// Why the grant was refused.
        refusal: Refusal,
    },
    /// The sign-in or the store refused; the service says why in its own words.
    Refused(Refusal),
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddError::Argument(what) => write!(f, "not understood: {what}"),
            AddError::DaemonRunning => f.write_str(
                "accountd is running and owns org.quire.Accounts1. It reads its registry only \
                 when it starts, so stop it, run this again and start it: \
                 systemctl --user stop accountd.service",
            ),
            AddError::UnknownProvider(id) => write!(
                f,
                "no provider `{}` is served (see the provider files)",
                id.as_str()
            ),
            AddError::AllowFailed { account, refusal } => write!(
                f,
                "account {} was added, but a grant was not: {refusal:?}",
                account.as_str()
            ),
            AddError::Refused(Refusal::Dismissed) => f.write_str("cancelled; nothing was added"),
            AddError::Refused(Refusal::Denied) => {
                f.write_str("the provider refused the sign-in; nothing was added")
            }
            AddError::Refused(other) => write!(f, "nothing was added: {other:?}"),
        }
    }
}

impl std::error::Error for AddError {}

/// The caller `accountd add` is: the person at a terminal, not an app.
fn person() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Accounts.Add")
            .unwrap_or_else(|_| unreachable!("org.quire.Accounts.Add is a well-formed app name")),
        isolation: Isolation::Unsandboxed,
    }
}

/// The need every Llm account of the providers meets: chat, any context.
fn llm_need() -> Need {
    Need::Llm(LlmNeed {
        features: BTreeSet::from([LlmFeature::Chat]),
        context: Tokens(0),
    })
}

/// Takes `org.quire.Accounts1` on `connection` so no daemon runs beside the command. A name that
/// is owned, or that another claimant is waiting on, is a daemon.
pub async fn take_the_name(connection: &zbus::Connection) -> Result<(), AddError> {
    let reply = connection
        .request_name_with_flags(ACCOUNTS_BUS, zbus::fdo::RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(()),
        _ => Err(AddError::DaemonRunning),
    }
}

/// Adds the account and gives it to the `--allow` apps. `service` is built over the daemon's own
/// seams with `sheets` as its sheet host.
pub async fn run<P, S, T, K, R, A>(
    service: &AccountService<P, S, TerminalSheets<T>, K, R, A>,
    sheets: &TerminalSheets<T>,
    served: &[ProviderId],
    args: &AddArgs,
) -> Result<AddReport, AddError>
where
    P: Provider,
    S: Secrets,
    T: Terminal,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
{
    if !served.contains(&args.provider) {
        return Err(AddError::UnknownProvider(args.provider.clone()));
    }
    let reply = service
        .handle(
            &person(),
            AccountsRequest::AddAccount {
                hint: ProviderHint::Provider(args.provider.clone()),
                window: ParentWindow::Unparented,
            },
        )
        .await;
    let account = match reply {
        AccountsReply::Added(id) => id,
        AccountsReply::Refused(refusal) => return Err(AddError::Refused(refusal)),
        _ => return Err(AddError::Refused(Refusal::Unavailable)),
    };
    sheets.allow(account.clone());
    // `--allow` gives an app an account for language models. An agent that signs itself in has
    // none to give (no model, no key), so its grants are not made; the account stays.
    let signs_itself_in = service
        .registry()
        .accounts
        .iter()
        .any(|a| a.id == account && a.auth == porter_core::AuthKind::AgentLogin);
    if signs_itself_in && !args.allow.is_empty() {
        return Err(AddError::AllowFailed {
            account,
            refusal: Refusal::NoFittingAccount,
        });
    }
    let mut grants = Vec::new();
    for app in &args.allow {
        for class in args.granted_classes() {
            let reply = service
                .handle(
                    app,
                    AccountsRequest::Choose {
                        need: llm_need(),
                        class,
                        usage: Usage::Interactive,
                        window: ParentWindow::Unparented,
                    },
                )
                .await;
            match reply {
                AccountsReply::Chosen(candidate) => {
                    grants.push((app.clone(), class, candidate.grant));
                }
                AccountsReply::Refused(refusal) => {
                    return Err(AddError::AllowFailed { account, refusal });
                }
                _ => {
                    return Err(AddError::AllowFailed {
                        account,
                        refusal: Refusal::Unavailable,
                    });
                }
            }
        }
    }
    Ok(AddReport { account, grants })
}
