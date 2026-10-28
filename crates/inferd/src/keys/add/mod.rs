//! `inferd add`: add an AI key from a terminal, for a computer with no sheet host yet.
//!
//! It runs the same `AccountService` add the sheet host drives, with a terminal in the sheet's
//! place (`terminal`): a pasted key is read with echo off, and the secret is filed through the
//! same secret store and registry the daemon uses. A key goes into the sign-in and nowhere else:
//! not onto a bus, not into a log.
//!
//! **Consent.** Each `--allow <app>` records the permit an `Allow, always` answer to that app's
//! consent sheet would: the person typing the command is the consent. The permit is for the new
//! account, for language models, interactive use, in every data class (or the `--class` ones),
//! and it is the whole of what the app is given: inferd still lowers nothing, and
//! `ai.floor.<class>` still decides which classes may leave this computer.
//!
//! **The daemon.** inferd reads its registry when it starts and writes it itself; two writers
//! would lose an update. So the command takes the bus name `org.quire.Inference1` for as long as
//! it runs, which is also the lock: a running daemon already owns the name and the command stops
//! with `AddError::DaemonRunning` (stop the unit, run the command, start the unit), and a daemon
//! that starts meanwhile cannot take the name, so it never reads half a registry. When there is
//! no session bus the command runs without the lock and says so.

mod terminal;

pub use terminal::{Echo, StdTerminal, Terminal, TerminalLink, TerminalSheets};

use super::audit::KeyAudit;
use super::build::{KeysStartError, load_families, load_registry, served_ids};
use crate::config::Dirs;
use porter_core::ProviderHint;
use porter_core::consent::Usage;
use porter_core::store::RegistryStore;
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{AppId, AppName, DataClass, Isolation};
use porter_dbus::BusTarget;
use porter_dbus::inference::INFERENCE_BUS;
use porter_ids::clock::Clock;
use porter_ids::clock::SystemClock;
use porter_provider::Provider;
use porter_secrets::Namespace;
use porter_secrets::Secrets;
use porter_secrets::keysel::{Chosen, Selection};
use porter_service::AccountService;
use porter_service_core::audit::AuditSink;
use porter_wire::{AccountsReply, AccountsRequest};
use porter_wire::{ParentWindow, Refusal};
use std::collections::BTreeSet;
use zbus::fdo::RequestNameReply;

/// Every data class, for `--allow` without `--class`.
pub const ALL_CLASSES: [DataClass; 13] = [
    DataClass::AppOwn,
    DataClass::Mail,
    DataClass::Calendar,
    DataClass::Contacts,
    DataClass::Tasks,
    DataClass::Notes,
    DataClass::Files,
    DataClass::Photos,
    DataClass::Clipboard,
    DataClass::Screen,
    DataClass::Voice,
    DataClass::Prompt,
    DataClass::Public,
];

fn slug<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// What the person asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddArgs {
    /// The provider file's id (`anthropic`, `openai`).
    pub provider: ProviderId,
    /// The apps to give the account to.
    pub allow: Vec<AppId>,
    /// The data classes to allow; every class when empty.
    pub classes: Vec<DataClass>,
}

impl AddArgs {
    /// The arguments as typed: a provider id, `--allow` apps as `name` or `name:isolation`
    /// (`org.quire.Companion`, `org.example.App:flatpak`; unsandboxed when none is given), and
    /// `--class` slugs.
    pub fn parse(provider: &str, allow: &[String], classes: &[String]) -> Result<Self, AddError> {
        let bad = |what: &str, text: &str| AddError::Argument(format!("{what} `{text}`"));
        let provider = ProviderId::parse(provider).map_err(|_| bad("provider id", provider))?;
        let allow = allow
            .iter()
            .map(|text| {
                let (name, isolation) = match text.split_once(':') {
                    Some((name, isolation)) => (name, slug::<Isolation>(isolation)),
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
            .map(|text| slug::<DataClass>(text).ok_or_else(|| bad("data class", text)))
            .collect::<Result<Vec<_>, AddError>>()?;
        Ok(Self {
            provider,
            allow,
            classes,
        })
    }

    fn allowed_classes(&self) -> Vec<DataClass> {
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
    /// Each permit made: the app, the class and the permit's id.
    pub permits: Vec<(AppId, DataClass, GrantId)>,
}

/// Why `inferd add` did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddError {
    /// An argument is not what it says.
    Argument(String),
    /// inferd owns `org.quire.Inference1`.
    DaemonRunning,
    /// No provider file of that id is served.
    UnknownProvider(ProviderId),
    /// The account was added, and an `--allow` permit after it was not.
    AllowFailed {
        /// The account that exists now.
        account: AccountId,
        /// Why the permit was refused.
        refusal: Refusal,
    },
    /// The sign-in or the store refused; the service says why in its own words.
    Refused(Refusal),
    /// The secret store or the registry could not be opened; the text says why.
    Start(String),
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddError::Argument(what) => write!(f, "not understood: {what}"),
            AddError::DaemonRunning => f.write_str(
                "inferd is running and owns org.quire.Inference1. It reads its keys only \
                 when it starts, so stop it, run this again and start it: \
                 systemctl --user stop inferd.service",
            ),
            AddError::UnknownProvider(id) => write!(
                f,
                "no provider `{}` is served (see the provider files)",
                id.as_str()
            ),
            AddError::AllowFailed { account, refusal } => write!(
                f,
                "account {} was added, but a permission was not given: {refusal:?}",
                account.as_str()
            ),
            AddError::Refused(Refusal::Dismissed) => f.write_str("cancelled; nothing was added"),
            AddError::Refused(Refusal::Denied) => {
                f.write_str("the provider refused the sign-in; nothing was added")
            }
            AddError::Refused(other) => write!(f, "nothing was added: {other:?}"),
            AddError::Start(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for AddError {}

impl From<KeysStartError> for AddError {
    fn from(why: KeysStartError) -> Self {
        AddError::Start(why.to_string())
    }
}

/// The caller `inferd add` is: the person at a terminal, not an app.
fn person() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Inference.Add")
            .unwrap_or_else(|_| unreachable!("org.quire.Inference.Add is a well-formed app name")),
        isolation: Isolation::Unsandboxed,
    }
}

/// Takes `org.quire.Inference1` on `connection` so no daemon runs beside the command. A name that
/// is owned, or that another claimant is waiting on, is a daemon.
pub async fn take_the_name(connection: &zbus::Connection) -> Result<(), AddError> {
    let reply = connection
        .request_name_with_flags(
            INFERENCE_BUS,
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        )
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
    // none to give (no model, no key), so its permits are not made; the account stays.
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
    let mut permits = Vec::new();
    for app in &args.allow {
        for class in args.allowed_classes() {
            let reply = service
                .handle(
                    app,
                    AccountsRequest::Choose {
                        need: crate::cloud::accountd::llm_need(),
                        class,
                        usage: Usage::Interactive,
                        window: ParentWindow::Unparented,
                    },
                )
                .await;
            match reply {
                AccountsReply::Chosen(candidate) => {
                    permits.push((app.clone(), class, candidate.grant));
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
    Ok(AddReport { account, permits })
}

/// `inferd add`: the provider files, the registry and the secret store the daemon uses, a
/// terminal for a sheet, and the bus name as the lock. `provider`, `allow` and `classes` are the
/// command's arguments as typed.
///
/// # Errors
/// See [`AddError`].
pub async fn add_account(
    dirs: &Dirs,
    selection: Selection,
    bus: &BusTarget,
    provider: &str,
    allow: &[String],
    classes: &[String],
) -> Result<AddReport, AddError> {
    let namespace = Namespace::parse(super::NAMESPACE)
        .unwrap_or_else(|_| unreachable!("porter.inferd is a well-formed namespace"));
    let chosen = Chosen::open(selection, namespace, "inferd").map_err(KeysStartError::from)?;
    eprintln!("{}", chosen.said);
    let args = AddArgs::parse(provider, allow, classes)?;
    let families = load_families(dirs);
    let served = served_ids(&families.families);
    // The lock. With no session bus no daemon can be reached either, so go on and say so.
    let session = bus.connect().await.ok();
    match &session {
        Some(connection) => take_the_name(connection).await?,
        None => eprintln!("inferd: no session bus; cannot tell whether the daemon is running"),
    }
    let store = porter_store::FileStore::new(dirs.keys.clone());
    let registry = load_registry(&store).await?;
    let sheets = TerminalSheets::new(StdTerminal);
    let service = AccountService::new(
        families.families,
        registry,
        chosen.keys,
        sheets.clone(),
        SystemClock,
    )
    .with_store(store)
    .with_audit(KeyAudit::new(dirs.keys_audit.clone()));
    run(&service, &sheets, &served, &args).await
}
