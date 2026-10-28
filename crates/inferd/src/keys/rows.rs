//! The rows of the AI accounts in inferd's settings module (`/org/quire/Inference1/settings`),
//! as a pure function of the registry: one row per account for its label, place, state, how it
//! signs in, its provider and group (read-outs), a switch for the language service, one Revoke
//! per permit with its day, how long it lasts, its app and its service, what an account reached
//! over the network spent this month and its monthly cap (when inferd meters hosted models), and
//! "Sign in" / "Sign out" / "Remove account" actions. The paths and words are the ones accountd's
//! rows had (`accounts.<id>.*`), so Settings reads them unchanged. Every key is under `accounts.`,
//! so it is never agent-settable (`ds-settings` refuses a schema that says otherwise).

use super::host::KeyHost;
use crate::cloud::accountd::Boxed;
use crate::cloud::spend::written;
use crate::cloud::wire::charges_of;
use ds_settings::live::{LiveError, Served};
use ds_settings::schema::{ActionWeight, Exposure, Help, KeyKind, KeyPath, KeySpec};
use iso_currency::Currency;
use model_catalog::ProviderId;
use model_provider::{Charges, Money};
use porter_capability::LoginOutcome;
use porter_capability::{CapabilityKind, Offer, Toggle};
use porter_core::consent::{Decision, Grant, GrantScope};
use porter_core::{Account, AccountState, AgentState, AppId, AppName, AuthKind, Isolation};
use porter_core::{AccountId, GrantId};
use porter_families::set::ProviderNames;
use porter_infer::ChargeTotal;
use porter_service_accounts::{Launchers as LaunchersSeam, LoginEnd, program_of};
use porter_service_core::registry::Registry;
use porter_settings::{Names, READOUTS, REAUTH, REMOVE, Row, action, off};
use porter_sheets::AppNames;
use porter_wire::AccountsReply;
use porter_wire::ParentWindow;
use rust_decimal::Decimal;
use std::sync::{Arc, OnceLock};

/// What a key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// `accounts.<id>.<row>`: a row every account has (`porter-settings`).
    Account(AccountId, Row),
    /// `accounts.<id>.service.<kind>`.
    Service(AccountId, CapabilityKind),
    /// `accounts.<id>.grant.<grant>`: a permit, with its Revoke action.
    Grant(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.since`: read-only, the UTC day the permit was given.
    Since(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.scope`: read-only, how long it holds.
    Scope(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.app`: read-only, the app the permit is for (its labels give
    /// the app's name).
    App(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.service`: read-only, the service it reaches (its labels give
    /// the service's name).
    GrantService(AccountId, GrantId),
    /// `accounts.<id>.spend.month`: read-only, what the provider reported was charged through the
    /// account on hosted models this calendar month (UTC), in whole US cents rounded up, as
    /// digits. A provider that reports no charges leaves it at `0` and the row says so.
    SpendMonth(AccountId),
    /// `accounts.<id>.spend.cap`: read-only, the monthly cap on an account
    /// (`ai.spend.account_monthly_cents`) in whole US cents, as digits; `0` for no cap. For a
    /// provider that reports no charges the row says, before anything runs, that the limit
    /// cannot apply.
    SpendCap(AccountId),
    /// `accounts.<id>.sign_out`: for an agent that signs itself in, forgets that it was signed in.
    SignOut(AccountId),
}

fn slug<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn from_slug<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// The key `path` names, among the accounts of `registry` (an id may hold dots, so the account is
/// found by prefix, the longest id first).
pub(crate) fn parse(path: &str, registry: &Registry) -> Option<Key> {
    let rest = path.strip_prefix("accounts.")?;
    let mut ids: Vec<&AccountId> = registry.accounts.iter().map(|a| &a.id).collect();
    ids.sort_by_key(|id| std::cmp::Reverse(id.as_str().len()));
    ids.into_iter().find_map(|id| {
        let tail = rest.strip_prefix(id.as_str())?.strip_prefix('.')?;
        let id = id.clone();
        if let Some(row) = Row::named(tail) {
            return Some(Key::Account(id, row));
        }
        match tail {
            "sign_out" => Some(Key::SignOut(id)),
            "spend.month" => Some(Key::SpendMonth(id)),
            "spend.cap" => Some(Key::SpendCap(id)),
            _ => {
                if let Some(kind) = tail.strip_prefix("service.") {
                    return from_slug(kind).map(|k| Key::Service(id, k));
                }
                let grant = tail.strip_prefix("grant.")?;
                // A permit's own rows end `.since`, `.scope`, `.app` and `.service`; an id may
                // hold dots, so the suffix counts only when what is left names a permit the
                // registry has.
                for (suffix, make) in [
                    (".since", Key::Since as fn(AccountId, GrantId) -> Key),
                    (".scope", Key::Scope),
                    (".app", Key::App),
                    (".service", Key::GrantService),
                ] {
                    let held = grant
                        .strip_suffix(suffix)
                        .and_then(|g| GrantId::parse(g).ok())
                        .filter(|g| registry.grants.iter().any(|held| held.id == *g));
                    if let Some(held) = held {
                        return Some(make(id, held));
                    }
                }
                GrantId::parse(grant).ok().map(|g| Key::Grant(id, g))
            }
        }
    })
}

/// The path of `key`.
pub(crate) fn path(key: &Key) -> String {
    match key {
        Key::Account(id, row) => row.path(id),
        Key::Service(id, kind) => format!("accounts.{id}.service.{}", slug(kind)),
        Key::Grant(id, grant) => format!("accounts.{id}.grant.{grant}"),
        Key::Since(id, grant) => format!("accounts.{id}.grant.{grant}.since"),
        Key::Scope(id, grant) => format!("accounts.{id}.grant.{grant}.scope"),
        Key::App(id, grant) => format!("accounts.{id}.grant.{grant}.app"),
        Key::GrantService(id, grant) => format!("accounts.{id}.grant.{grant}.service"),
        Key::SpendMonth(id) => format!("accounts.{id}.spend.month"),
        Key::SpendCap(id) => format!("accounts.{id}.spend.cap"),
        Key::SignOut(id) => format!("accounts.{id}.sign_out"),
    }
}

fn spec(
    key: &Key,
    section: &str,
    label: String,
    help: &str,
    kind: KeyKind,
    default: toml::Value,
) -> KeySpec {
    porter_settings::spec(
        path(key),
        section,
        label,
        help,
        kind,
        default,
        Exposure::Basic,
    )
}

/// A permit's row: "Companion can use Language model" (allowed) or "Mail can't use Language
/// model" (refused), and "... (this session)" for one that lasts one launcher session.
fn grant_label(grant: &Grant, names: &AppNames) -> String {
    let can = match grant.decision {
        Decision::Allow => "can",
        Decision::Deny => "can't",
    };
    let lasting = match grant.scope {
        GrantScope::Session(_) => " (this session)",
        GrantScope::Once | GrantScope::Always => "",
    };
    format!(
        "{} {can} use {}{lasting}",
        names.title_of(&grant.key.app.name).0,
        grant.key.kind.name()
    )
}

/// What the `since` row of `grant` says: the UTC day it was given, `YYYY-MM-DD`.
pub(crate) fn since_text(grant: &Grant) -> String {
    grant.at.utc_date().unwrap_or_default()
}

/// A read-out row whose one value is `word`, as a person reads it by `labels`.
fn readout(key: &Key, section: &str, label: &str, word: &str, labels: &[(&str, &str)]) -> KeySpec {
    porter_settings::readout(path(key), section, label, word, labels)
}

/// `spec` with the help text under its row.
fn with_help(mut spec: KeySpec, help: &str) -> KeySpec {
    spec.help = Help(help.to_owned());
    spec
}

/// How long a permit holds, as a person reads it.
const SCOPE_WORDS: [(&str, &str); 3] = [
    ("once", "Once"),
    ("always", "Always"),
    ("session", "This session only"),
];

fn account_keys(
    account: &Account,
    grants: &[Grant],
    names: &AppNames,
    providers: &ProviderNames,
    spending: bool,
) -> Vec<KeySpec> {
    let section = account.label.0.as_str();
    let id = &account.id;
    let provider_label = providers
        .label_of(&account.provider)
        .unwrap_or_else(|| account.provider.as_str());
    let shared = Names {
        provider_label,
        group: providers.group_of(account),
    };
    let mut keys: Vec<KeySpec> = READOUTS
        .iter()
        .filter_map(|row| row.0.spec(account, section, &shared))
        .collect();
    // An account with several models holds one claim per model of the same kind: one switch.
    let mut toggles: Vec<CapabilityKind> = Vec::new();
    for kind in account
        .capabilities
        .iter()
        .filter_map(|claim| match &claim.offer {
            Offer::Present(_) => Some(claim.offer.kind()),
            Offer::Absent {
                kind,
                reason: porter_capability::AbsentReason::TurnedOff,
            } => Some(*kind),
            Offer::Absent { .. } => None,
        })
    {
        if !toggles.contains(&kind) {
            toggles.push(kind);
        }
    }
    for kind in toggles {
        keys.push(spec(
            &Key::Service(id.clone(), kind),
            section,
            kind.name().to_owned(),
            "Whether apps may use this service of the account.",
            KeyKind::Toggle {
                variants: ["on".to_owned(), "off".to_owned()],
            },
            toml::Value::String("on".to_owned()),
        ));
    }
    for grant in grants.iter().filter(|g| g.key.account == *id) {
        keys.push(spec(
            &Key::Grant(id.clone(), grant.id.clone()),
            section,
            grant_label(grant, names),
            "Revoking asks the app again the next time it needs the account.",
            action("Revoke", ActionWeight::Plain),
            off(),
        ));
        keys.push(readout(
            &Key::Since(id.clone(), grant.id.clone()),
            section,
            "Since",
            &since_text(grant),
            &[],
        ));
        keys.push(readout(
            &Key::Scope(id.clone(), grant.id.clone()),
            section,
            "Lasts",
            grant.scope.word(),
            &SCOPE_WORDS,
        ));
        let app = grant.key.app.name.as_str();
        keys.push(readout(
            &Key::App(id.clone(), grant.id.clone()),
            section,
            "App",
            app,
            &[(app, &names.title_of(&grant.key.app.name).0)],
        ));
        let service = slug(&grant.key.kind);
        keys.push(readout(
            &Key::GrantService(id.clone(), grant.id.clone()),
            section,
            "Service",
            &service,
            &[(&service, grant.key.kind.name())],
        ));
    }
    // A program on this computer costs nothing: only an account reached over the network spends.
    if spending && account.auth != AuthKind::LocalRuntime {
        // Only what a provider reports is counted. For one that reports nothing, both rows say so
        // before any request runs: its replies show tokens and never dollars, so a limit on it
        // could never be met.
        let reported = charges_of(&ProviderId(account.provider.as_str().to_owned()));
        let says = |what: &str| {
            format!("{provider_label} doesn't report what its replies cost, so {what}.")
        };
        let (counted, limit) = match reported {
            Charges::PerReply => (String::new(), String::new()),
            _ => (
                says("nothing is counted here"),
                says("a limit can't apply to it"),
            ),
        };
        keys.push(with_help(
            readout(
                &Key::SpendMonth(id.clone()),
                section,
                "Spent this month",
                "0",
                &[],
            ),
            &counted,
        ));
        keys.push(with_help(
            readout(
                &Key::SpendCap(id.clone()),
                section,
                "Monthly limit",
                "0",
                &[],
            ),
            &limit,
        ));
    }
    keys.extend(REAUTH.0.spec(account, section, &shared));
    // An agent that is signed in signs itself out from here; one that is signed out signs in.
    if account.auth == AuthKind::AgentLogin && account.state != AccountState::NeedsLogin {
        keys.push(spec(
            &Key::SignOut(id.clone()),
            section,
            "Sign out".to_owned(),
            "Asks the assistant to sign itself out when it is running; otherwise it only forgets here that it was signed in, and the assistant's own login is not touched.",
            action("Sign out", ActionWeight::Plain),
            off(),
        ));
    }
    keys.extend(REMOVE.0.spec(account, section, &shared));
    keys
}

/// The rows of every account of `registry`; with `spending`, each account reached over the
/// network has its spend this month and its monthly cap too.
pub(crate) fn schema(
    registry: &Registry,
    names: &AppNames,
    providers: &ProviderNames,
    spending: bool,
) -> Vec<KeySpec> {
    registry
        .accounts
        .iter()
        .flat_map(|account| account_keys(account, &registry.grants, names, providers, spending))
        .collect()
}

/// What accounts have spent on hosted models this month, and the cap on each.
pub trait SpendBook: Send + Sync + 'static {
    /// What was spent through `account` this calendar month (UTC), and the monthly cap on an
    /// account (`None`: no cap).
    fn month(&self, account: &AccountId) -> (ChargeTotal, Option<Money>);
}

/// `money` as the spend rows show it: US dollars in whole cents, as digits, rounded up so a part
/// of a cent shows as one; an amount in another currency as its digits and code (`3 EUR`).
fn cents_up(money: Money) -> String {
    if money.currency != Currency::USD {
        return written(money);
    }
    money
        .amount
        .checked_mul(Decimal::ONE_HUNDRED)
        .map_or_else(|| written(money), |cents| cents.ceil().to_string())
}

/// The rows of the AI accounts, as the settings module serves them.
pub trait KeyRows: Send + Sync + 'static {
    /// Every row, for the schema.
    fn schema(&self) -> Vec<KeySpec>;

    /// Whether `path` is one of these rows.
    fn handles(&self, path: &str) -> bool;

    /// The value of a row.
    fn get<'a>(&'a self, path: &'a str) -> Boxed<'a, Result<toml::Value, LiveError>>;

    /// Changes a row (an action row is pressed with any value).
    fn set<'a>(&'a self, path: &'a str, value: toml::Value) -> Boxed<'a, Result<(), LiveError>>;

    /// Gives the rows the served module, which announces what changed.
    fn bind(&self, served: Served);
}

/// Where the served module is kept once it exists, shared with whoever announces changes.
pub type ServedSlot = Arc<OnceLock<Served>>;

/// How a "Sign in again" pressed in Settings ended, as the word `accounts.<id>.reauth` changes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReauthNews {
    Done,
    Cancelled,
    Refused,
    NoLauncher,
    Failed,
}

impl ReauthNews {
    fn of(reply: &AccountsReply) -> Self {
        use porter_wire::Refusal;
        match reply {
            AccountsReply::Reauthenticated => ReauthNews::Done,
            AccountsReply::Refused(Refusal::Dismissed) => ReauthNews::Cancelled,
            AccountsReply::Refused(Refusal::Denied) => ReauthNews::Refused,
            AccountsReply::Refused(Refusal::NoLauncher) => ReauthNews::NoLauncher,
            _ => ReauthNews::Failed,
        }
    }

    fn slug(self) -> &'static str {
        match self {
            ReauthNews::Done => "done",
            ReauthNews::Cancelled => "cancelled",
            ReauthNews::Refused => "refused",
            ReauthNews::NoLauncher => "no_launcher",
            ReauthNews::Failed => "failed",
        }
    }
}

/// How a sign out ended, as the word `accounts.<id>.sign_out` changes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignOutNews {
    Asked,
    LoginUntouched,
    Done,
    Failed,
    Cancelled,
    Expired,
    LauncherGone,
}

impl SignOutNews {
    fn slug(self) -> &'static str {
        match self {
            SignOutNews::Asked => "asked",
            SignOutNews::LoginUntouched => "login_untouched",
            SignOutNews::Done => "done",
            SignOutNews::Failed => "failed",
            SignOutNews::Cancelled => "cancelled",
            SignOutNews::Expired => "expired",
            SignOutNews::LauncherGone => "launcher_gone",
        }
    }

    fn of(end: LoginEnd) -> Self {
        match end {
            LoginEnd::Reported(LoginOutcome::Ready) => SignOutNews::Done,
            LoginEnd::Reported(LoginOutcome::Cancelled) => SignOutNews::Cancelled,
            LoginEnd::Expired => SignOutNews::Expired,
            LoginEnd::LauncherGone => SignOutNews::LauncherGone,
            _ => SignOutNews::Failed,
        }
    }
}

/// The app Settings-driven sign-ins are run as.
fn settings_app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Settings").expect("a literal app name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn failed(why: impl ToString) -> LiveError {
    LiveError::Failed(why.to_string())
}

fn word(value: &toml::Value) -> Option<Toggle> {
    match value {
        toml::Value::String(s) if s == "on" => Some(Toggle::On),
        toml::Value::String(s) if s == "off" => Some(Toggle::Off),
        toml::Value::Boolean(true) => Some(Toggle::On),
        toml::Value::Boolean(false) => Some(Toggle::Off),
        _ => None,
    }
}

/// The rows over the hosted service `H`, its launchers `L` and the names a person reads.
pub struct AccountRows<H, L> {
    host: Arc<H>,
    launchers: L,
    names: AppNames,
    providers: ProviderNames,
    served: ServedSlot,
    spend: Option<Arc<dyn SpendBook>>,
}

impl<H, L> std::fmt::Debug for AccountRows<H, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountRows").finish_non_exhaustive()
    }
}

impl<H, L> AccountRows<H, L> {
    /// The rows of `host`'s accounts; `launchers` sign an agent in and out; `served` is where
    /// the served module is kept (shared with the news watcher).
    pub fn new(
        host: Arc<H>,
        launchers: L,
        names: AppNames,
        providers: ProviderNames,
        served: ServedSlot,
    ) -> Self {
        Self {
            host,
            launchers,
            names,
            providers,
            served,
            spend: None,
        }
    }

    /// The rows with each account's spend this month and its monthly cap, read from `spend`.
    pub fn with_spend(self, spend: Arc<dyn SpendBook>) -> Self {
        Self {
            spend: Some(spend),
            ..self
        }
    }
}

/// Tells the module's listeners `Changed(<the key's path>, <word>)`.
async fn announce(served: &ServedSlot, key: &Key, word: &str) {
    if let Some(module) = served.get() {
        let _ = module
            .changed(&KeyPath(path(key)), &toml::Value::String(word.to_owned()))
            .await;
    }
}

impl<H: KeyHost, L: LaunchersSeam + Clone + 'static> AccountRows<H, L> {
    async fn sign_out(&self, id: &AccountId) -> Result<(), LiveError> {
        let program = self
            .host
            .registry()
            .accounts
            .iter()
            .find(|a| a.id == *id && a.auth == AuthKind::AgentLogin)
            .and_then(program_of);
        let waiting = match program {
            Some(program) => self.launchers.ask_logout(id, &program).await.ok(),
            None => None,
        };
        let Some(waiting) = waiting else {
            self.host
                .set_agent_state(id, AgentState::NeedsLogin)
                .await
                .map_err(|fault| {
                    eprintln!("inferd: signing an assistant out was refused: {fault:?}");
                    failed("That could not be done.")
                })?;
            announce(
                &self.served,
                &Key::SignOut(id.clone()),
                SignOutNews::LoginUntouched.slug(),
            )
            .await;
            return Ok(());
        };
        announce(
            &self.served,
            &Key::SignOut(id.clone()),
            SignOutNews::Asked.slug(),
        )
        .await;
        let (served, id) = (Arc::clone(&self.served), id.clone());
        tokio::spawn(async move {
            let news = SignOutNews::of(waiting.await);
            announce(&served, &Key::SignOut(id), news.slug()).await;
        });
        Ok(())
    }

    async fn read(&self, path_text: &str) -> Result<toml::Value, LiveError> {
        let registry = self.host.registry();
        let unknown = || LiveError::UnknownKey(path_text.to_owned());
        let account = |id: &AccountId| {
            registry
                .accounts
                .iter()
                .find(|a| a.id == *id)
                .ok_or_else(unknown)
        };
        let text = |t: &str| Ok(toml::Value::String(t.to_owned()));
        match parse(path_text, &registry).ok_or_else(unknown)? {
            Key::Service(id, kind) => {
                let on = account(&id)?
                    .capabilities
                    .iter()
                    .any(|c| c.offer.kind() == kind && matches!(c.offer, Offer::Present(_)));
                text(if on { "on" } else { "off" })
            }
            Key::Account(id, row) => {
                let account = account(&id)?;
                let names = Names {
                    provider_label: self
                        .providers
                        .label_of(&account.provider)
                        .unwrap_or_else(|| account.provider.as_str()),
                    group: self.providers.group_of(account),
                };
                Ok(row.0.read(account, &names))
            }
            Key::Since(_, grant) => registry
                .grants
                .iter()
                .find(|g| g.id == grant)
                .map(|g| toml::Value::String(since_text(g)))
                .ok_or_else(unknown),
            Key::Scope(_, grant) => registry
                .grants
                .iter()
                .find(|g| g.id == grant)
                .map(|g| toml::Value::String(g.scope.word().to_owned()))
                .ok_or_else(unknown),
            Key::App(_, grant) => registry
                .grants
                .iter()
                .find(|g| g.id == grant)
                .map(|g| toml::Value::String(g.key.app.name.as_str().to_owned()))
                .ok_or_else(unknown),
            Key::GrantService(_, grant) => registry
                .grants
                .iter()
                .find(|g| g.id == grant)
                .map(|g| toml::Value::String(slug(&g.key.kind)))
                .ok_or_else(unknown),
            Key::SpendMonth(id) => {
                let spend = self.spend.as_ref().ok_or_else(unknown)?;
                account(&id)?;
                text(&cents_up(spend.month(&id).0.reported))
            }
            Key::SpendCap(id) => {
                let spend = self.spend.as_ref().ok_or_else(unknown)?;
                account(&id)?;
                let cap = spend.month(&id).1;
                text(&cap.map_or_else(|| "0".to_owned(), cents_up))
            }
            Key::Grant(..) | Key::SignOut(_) => Ok(toml::Value::Boolean(false)),
        }
    }

    async fn write(&self, path_text: &str, value: toml::Value) -> Result<(), LiveError> {
        let registry = self.host.registry();
        let parsed = parse(path_text, &registry)
            .ok_or_else(|| LiveError::UnknownKey(path_text.to_owned()))?;
        let refused = |r: porter_wire::Refusal| {
            eprintln!("inferd: a Settings change was refused: {r:?}");
            failed("That could not be done.")
        };
        match parsed {
            Key::Service(id, kind) => {
                let toggle = word(&value)
                    .ok_or_else(|| LiveError::BadValue("a service is `on` or `off`".into()))?;
                self.host
                    .set_toggle(&id, kind, toggle)
                    .await
                    .map_err(refused)?;
            }
            Key::Grant(_, grant) => {
                if !registry.grants.iter().any(|g| g.id == grant) {
                    return Err(LiveError::UnknownKey(path_text.to_owned()));
                }
                self.host.revoke_grant(&grant).await.map_err(refused)?;
            }
            Key::Account(id, row) if row == REMOVE => {
                self.host.remove(&id).await.map_err(failed)?;
            }
            Key::Account(id, row) if row == REAUTH => {
                let local = registry.accounts.iter().any(|a| {
                    a.id == id && matches!(a.auth, AuthKind::LocalRuntime | AuthKind::None)
                });
                if local {
                    return Err(LiveError::NotPermitted(
                        "this account has nothing to sign in to".into(),
                    ));
                }
                let (host, launchers, served) = (
                    Arc::clone(&self.host),
                    self.launchers.clone(),
                    Arc::clone(&self.served),
                );
                let agent = registry
                    .accounts
                    .iter()
                    .any(|a| a.id == id && a.auth == AuthKind::AgentLogin);
                tokio::spawn(async move {
                    let reply = match agent {
                        true => {
                            host.login_agent_any(
                                &settings_app(),
                                &id,
                                ParentWindow::Unparented,
                                &launchers,
                            )
                            .await
                        }
                        false => {
                            host.reauthenticate_any(&settings_app(), &id, ParentWindow::Unparented)
                                .await
                        }
                    };
                    if let AccountsReply::Refused(why) = &reply {
                        eprintln!("inferd: sign in again from Settings refused: {why:?}");
                    }
                    announce(
                        &served,
                        &Key::Account(id, REAUTH),
                        ReauthNews::of(&reply).slug(),
                    )
                    .await;
                });
            }
            Key::SignOut(id) => self.sign_out(&id).await?,
            Key::Account(..)
            | Key::Since(..)
            | Key::Scope(..)
            | Key::App(..)
            | Key::GrantService(..)
            | Key::SpendMonth(_)
            | Key::SpendCap(_) => {
                return Err(LiveError::NotPermitted("this row is a read-out".into()));
            }
        }
        Ok(())
    }
}

impl<H: KeyHost, L: LaunchersSeam + Clone + 'static> KeyRows for AccountRows<H, L> {
    fn schema(&self) -> Vec<KeySpec> {
        schema(
            &self.host.registry(),
            &self.names,
            &self.providers,
            self.spend.is_some(),
        )
    }

    fn handles(&self, path_text: &str) -> bool {
        parse(path_text, &self.host.registry()).is_some()
    }

    fn get<'a>(&'a self, path_text: &'a str) -> Boxed<'a, Result<toml::Value, LiveError>> {
        Box::pin(self.read(path_text))
    }

    fn set<'a>(
        &'a self,
        path_text: &'a str,
        value: toml::Value,
    ) -> Boxed<'a, Result<(), LiveError>> {
        Box::pin(self.write(path_text, value))
    }

    fn bind(&self, served: Served) {
        // Served once; a second call leaves the first in place.
        let _ = self.served.set(served);
    }
}

#[cfg(test)]
mod tests;
