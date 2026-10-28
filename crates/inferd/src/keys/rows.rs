//! The rows of the AI accounts in inferd's settings module (`/org/quire/Inference1/settings`),
//! as a pure function of the registry: one row per account for its label, place, state, how it
//! signs in, its provider and group (read-outs), a switch for the language service, one Revoke
//! per permit with its day, how long it lasts, its app and its service, what an account reached
//! over the network spent this month and its monthly cap (when inferd meters hosted models), and
//! "Sign in" / "Sign out" / "Remove account" actions. The paths and words are the ones accountd's
//! rows had (`accounts.<id>.*`), so Settings reads them unchanged. Every key is under `accounts.`,
//! so it is never agent-settable (`ds-settings` refuses a schema that says otherwise).

use super::host::KeyHost;
use super::local::state_slug;
use crate::cloud::accountd::Boxed;
use ds_settings::live::{LiveError, Served};
use ds_settings::schema::{
    ActionLabel, ActionWeight, AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label,
    LiveAction, Page, Section, WordLabels,
};
use porter_core::consent::{Decision, Grant, GrantScope};
use porter_core::sheet::ProviderGroup;
use porter_core::wire::ParentWindow;
use porter_core::{
    Account, AccountState, AccountsReply, AgentState, AppId, AppName, AuthKind, CapabilityKind,
    Isolation, LoginOutcome, Offer, Toggle,
};
use porter_core::{AccountId, GrantId};
use porter_families::set::ProviderNames;
use porter_ids::MicroUsd;
use porter_service::{Launchers as LaunchersSeam, LoginEnd, Registry, program_of};
use porter_sheets::AppNames;
use std::sync::{Arc, OnceLock};

/// What a key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// `accounts.<id>.service.<kind>`.
    Service(AccountId, CapabilityKind),
    /// `accounts.<id>.state`.
    State(AccountId),
    /// `accounts.<id>.label`.
    Label(AccountId),
    /// `accounts.<id>.place`.
    Place(AccountId),
    /// `accounts.<id>.sign_in`: read-only, how the account is signed in again.
    SignIn(AccountId),
    /// `accounts.<id>.provider`: read-only, the provider's id (the row's labels give its name).
    Provider(AccountId),
    /// `accounts.<id>.group`: read-only, the part of the Accounts page the account is listed under.
    Group(AccountId),
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
    /// `accounts.<id>.spend.month`: read-only, what was spent through the account on hosted
    /// models this calendar month (UTC), in whole US cents rounded up, as digits.
    SpendMonth(AccountId),
    /// `accounts.<id>.spend.cap`: read-only, the monthly cap on an account
    /// (`ai.spend.account_monthly_cents`) in whole US cents, as digits; `0` for no cap.
    SpendCap(AccountId),
    /// `accounts.<id>.reauth`.
    Reauth(AccountId),
    /// `accounts.<id>.sign_out`: for an agent that signs itself in, forgets that it was signed in.
    SignOut(AccountId),
    /// `accounts.<id>.remove`.
    Remove(AccountId),
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
        match tail {
            "state" => Some(Key::State(id)),
            "label" => Some(Key::Label(id)),
            "place" => Some(Key::Place(id)),
            "sign_in" => Some(Key::SignIn(id)),
            "provider" => Some(Key::Provider(id)),
            "group" => Some(Key::Group(id)),
            "reauth" => Some(Key::Reauth(id)),
            "sign_out" => Some(Key::SignOut(id)),
            "remove" => Some(Key::Remove(id)),
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
        Key::Service(id, kind) => format!("accounts.{id}.service.{}", slug(kind)),
        Key::State(id) => format!("accounts.{id}.state"),
        Key::Label(id) => format!("accounts.{id}.label"),
        Key::Place(id) => format!("accounts.{id}.place"),
        Key::SignIn(id) => format!("accounts.{id}.sign_in"),
        Key::Provider(id) => format!("accounts.{id}.provider"),
        Key::Group(id) => format!("accounts.{id}.group"),
        Key::Grant(id, grant) => format!("accounts.{id}.grant.{grant}"),
        Key::Since(id, grant) => format!("accounts.{id}.grant.{grant}.since"),
        Key::Scope(id, grant) => format!("accounts.{id}.grant.{grant}.scope"),
        Key::App(id, grant) => format!("accounts.{id}.grant.{grant}.app"),
        Key::GrantService(id, grant) => format!("accounts.{id}.grant.{grant}.service"),
        Key::SpendMonth(id) => format!("accounts.{id}.spend.month"),
        Key::SpendCap(id) => format!("accounts.{id}.spend.cap"),
        Key::Reauth(id) => format!("accounts.{id}.reauth"),
        Key::SignOut(id) => format!("accounts.{id}.sign_out"),
        Key::Remove(id) => format!("accounts.{id}.remove"),
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
    KeySpec {
        control: None,
        path: KeyPath(path(key)),
        kind,
        default,
        label: Label(label),
        help: Help(help.to_owned()),
        page: Page::Accounts,
        section: Section(section.to_owned()),
        exposure: Exposure::Basic,
        labels: Default::default(),
        unavailable: Default::default(),
        groups: Default::default(),
        places: Default::default(),
        note: None,
        agent: AgentSetting::HandsOff,
    }
}

fn action(name: &str, weight: ActionWeight) -> KeyKind {
    KeyKind::Live {
        action: LiveAction {
            label: ActionLabel(name.to_owned()),
            weight,
        },
    }
}

fn off() -> toml::Value {
    toml::Value::Boolean(false)
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
        grant.key.kind.display_name()
    )
}

/// What the `since` row of `grant` says: the UTC day it was given, `YYYY-MM-DD`.
pub(crate) fn since_text(grant: &Grant) -> String {
    grant.at.utc_date().unwrap_or_default()
}

fn labelled(mut spec: KeySpec, words: &[(&str, &str)]) -> KeySpec {
    spec.labels = WordLabels(
        words
            .iter()
            .map(|(word, label)| ((*word).to_owned(), (*label).to_owned()))
            .collect(),
    );
    spec
}

/// A read-out row whose one value is `word`, as a person reads it by `labels`.
fn readout(key: &Key, section: &str, label: &str, word: &str, labels: &[(&str, &str)]) -> KeySpec {
    labelled(
        spec(
            key,
            section,
            label.to_owned(),
            "",
            KeyKind::Fixed {
                variant: word.to_owned(),
            },
            toml::Value::String(word.to_owned()),
        ),
        labels,
    )
}

/// How long a permit holds, as a person reads it.
const SCOPE_WORDS: [(&str, &str); 3] = [
    ("once", "Once"),
    ("always", "Always"),
    ("session", "This session only"),
];

/// Where an account lives: a program on this computer, or anywhere else.
pub(crate) fn place_slug(account: &Account) -> &'static str {
    match account.auth {
        AuthKind::LocalRuntime => "this_computer",
        _ => "elsewhere",
    }
}

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
    let group_words: Vec<(&str, &str)> = ProviderGroup::ALL
        .iter()
        .map(|group| (group.slug(), group.display_name()))
        .collect();
    let fixed = |key: Key, label: &str, word: &str| {
        spec(
            &key,
            section,
            label.to_owned(),
            "",
            KeyKind::Fixed {
                variant: word.to_owned(),
            },
            toml::Value::String(word.to_owned()),
        )
    };
    let mut keys = vec![
        fixed(Key::Label(id.clone()), "Account", &account.label.0),
        fixed(Key::Place(id.clone()), "Where", place_slug(account)),
        fixed(Key::State(id.clone()), "Status", state_slug(account.state)),
        fixed(
            Key::SignIn(id.clone()),
            "Signs in",
            account.auth.sign_in_way().slug(),
        ),
        readout(
            &Key::Provider(id.clone()),
            section,
            "Account type",
            account.provider.as_str(),
            &[(account.provider.as_str(), provider_label)],
        ),
        readout(
            &Key::Group(id.clone()),
            section,
            "Listed under",
            providers.group_of(account).slug(),
            &group_words,
        ),
    ];
    // An account with several models holds one claim per model of the same kind: one switch.
    let mut toggles: Vec<CapabilityKind> = Vec::new();
    for kind in account
        .capabilities
        .iter()
        .filter_map(|claim| match &claim.offer {
            Offer::Present(_) => Some(claim.offer.kind()),
            Offer::Absent {
                kind,
                reason: porter_core::AbsentReason::TurnedOff,
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
            kind.display_name().to_owned(),
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
            &[(&service, grant.key.kind.display_name())],
        ));
    }
    // A program on this computer costs nothing: only an account reached over the network spends.
    if spending && account.auth != AuthKind::LocalRuntime {
        keys.push(readout(
            &Key::SpendMonth(id.clone()),
            section,
            "Spent this month",
            "0",
            &[],
        ));
        keys.push(readout(
            &Key::SpendCap(id.clone()),
            section,
            "Monthly limit",
            "0",
            &[],
        ));
    }
    keys.extend(match account.auth {
        AuthKind::AgentLogin if account.state == AccountState::NeedsLogin => Some(spec(
            &Key::Reauth(id.clone()),
            section,
            "Sign in".to_owned(),
            "Asks the assistant to sign itself in, which needs its app to be running.",
            action("Sign in", ActionWeight::Plain),
            off(),
        )),
        AuthKind::AgentLogin => Some(spec(
            &Key::SignOut(id.clone()),
            section,
            "Sign out".to_owned(),
            "Asks the assistant to sign itself out when it is running; otherwise it only forgets here that it was signed in, and the assistant's own login is not touched.",
            action("Sign out", ActionWeight::Plain),
            off(),
        )),
        // A program on this computer signs in with nothing: there is nothing to sign in to.
        AuthKind::LocalRuntime | AuthKind::None => None,
        _ => Some(spec(
            &Key::Reauth(id.clone()),
            section,
            "Sign in again".to_owned(),
            "",
            action("Sign in again", ActionWeight::Plain),
            off(),
        )),
    });
    keys.push(spec(
        &Key::Remove(id.clone()),
        section,
        "Remove account".to_owned(),
        "Deletes the account's stored key and every permission given for it.",
        action("Remove account", ActionWeight::Destructive),
        off(),
    ));
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
    fn month(&self, account: &AccountId) -> (MicroUsd, Option<MicroUsd>);
}

/// Micro-dollars in a cent.
const MICRO_USD_PER_CENT: u64 = 10_000;

/// `micro` in whole cents, as digits: rounded up, so a part of a cent spent shows as one.
fn cents_up(micro: MicroUsd) -> String {
    micro.0.div_ceil(MICRO_USD_PER_CENT).to_string()
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
        use porter_core::wire::Refusal;
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
            Key::State(id) => text(state_slug(account(&id)?.state)),
            Key::Label(id) => text(&account(&id)?.label.0),
            Key::Place(id) => text(place_slug(account(&id)?)),
            Key::SignIn(id) => text(account(&id)?.auth.sign_in_way().slug()),
            Key::Provider(id) => text(account(&id)?.provider.as_str()),
            Key::Group(id) => text(self.providers.group_of(account(&id)?).slug()),
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
                text(&cents_up(spend.month(&id).0))
            }
            Key::SpendCap(id) => {
                let spend = self.spend.as_ref().ok_or_else(unknown)?;
                account(&id)?;
                let cap = spend.month(&id).1;
                text(&cap.map_or_else(|| "0".to_owned(), cents_up))
            }
            Key::Grant(..) | Key::Reauth(_) | Key::SignOut(_) | Key::Remove(_) => {
                Ok(toml::Value::Boolean(false))
            }
        }
    }

    async fn write(&self, path_text: &str, value: toml::Value) -> Result<(), LiveError> {
        let registry = self.host.registry();
        let parsed = parse(path_text, &registry)
            .ok_or_else(|| LiveError::UnknownKey(path_text.to_owned()))?;
        let refused = |r: porter_core::wire::Refusal| {
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
            Key::Remove(id) => {
                self.host.remove(&id).await.map_err(failed)?;
            }
            Key::Reauth(id) => {
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
                    announce(&served, &Key::Reauth(id), ReauthNews::of(&reply).slug()).await;
                });
            }
            Key::SignOut(id) => self.sign_out(&id).await?,
            Key::State(_)
            | Key::Label(_)
            | Key::Place(_)
            | Key::SignIn(_)
            | Key::Provider(_)
            | Key::Group(_)
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
