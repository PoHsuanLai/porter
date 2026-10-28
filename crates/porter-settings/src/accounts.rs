//! The keys of accountd's settings module (PLAN §2.5), as a pure function of the registry: for
//! each account of every group (the identities and Tailscale, the AI services, the programs on
//! this computer and the assistants) the rows every account has ([`crate::ROWS`]); for an
//! identity the expiry of a sign-in that expires and the apps using it (`used_by.<n>`,
//! read-outs); for an AI account or an assistant a switch for each service, and one Revoke per
//! permit with its day, how long it lasts, its app and its service; and one text row per OAuth
//! issuer for a bring-your-own client id. Sync switches are syncd's. What an account spent on
//! hosted models is inferd's (`spend.month` and `spend.cap`, whose paths this module spells so
//! that both modules name the same account). Every key is under `accounts.`, so it is never
//! agent-settable (`ds-settings` refuses a schema that says otherwise).

use crate::{ACTIONS, Names, READOUTS, Row, action, off, readout as readout_of, spec as spec_of};
use ds_settings::live::LiveSchema;
use ds_settings::schema::{ActionWeight, Exposure, KeyKind, KeySpec};
use porter_capability::{AbsentReason, CapabilityKind, Offer};
use porter_core::consent::{Decision, Grant, GrantScope};
use porter_core::{Account, AccountId, AccountState, AppName, GrantId, ProviderId};
use porter_ids::word::{from_word, word};
use porter_provider::Issuer;
use porter_service_core::registry::Registry;
use porter_sheet::ProviderGroup;
use std::collections::BTreeMap;

/// What the daemon knows that the registry does not: how a provider and an app are called, and
/// where an account is listed.
pub trait Directory {
    /// What a person reads for `provider`, when its file is loaded.
    fn provider_label(&self, provider: &ProviderId) -> Option<&str>;
    /// The group of the page `account` is listed under.
    fn group_of(&self, account: &Account) -> ProviderGroup;
    /// What a person reads for `app`.
    fn app_title(&self, app: &AppName) -> String;
}

/// The issuers a bring-your-own client id may be set for. The writer changes only the id (and the
/// secret) of a row, so a hand-written row's other lines (Google's `testing`, `byo`) are kept.
pub const ISSUERS: &[Issuer] = &[Issuer::Microsoft, Issuer::Google];

/// The issuers whose client also needs its application secret (Google's desktop clients): they
/// get a second row, `accounts.clients.<issuer>.secret`.
pub const SECRET_ISSUERS: &[Issuer] = &[Issuer::Google];

/// The apps that use each identity, by the account it is (`Identities1.RegisterUse`).
pub type UsedBy = BTreeMap<AccountId, Vec<AppName>>;

/// What a key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// `accounts.<id>.<row>`: a row every account has.
    Account(AccountId, Row),
    /// `accounts.<id>.expires`: read-only, only for a sign-in that expires (Google, a client in
    /// testing).
    Expires(AccountId),
    /// `accounts.<id>.used_by.<n>`: read-only, the display name of the n-th app (from 0, in name
    /// order) that signed in with this identity.
    UsedBy(AccountId, usize),
    /// `accounts.<id>.service.<kind>`: the switch for one service of an AI account or an
    /// assistant.
    Service(AccountId, CapabilityKind),
    /// `accounts.<id>.grant.<grant>`: a permit, with its Revoke action.
    Grant(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.since`: read-only, the UTC day the permit was given.
    Since(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.scope`: read-only, how long it holds.
    Scope(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.app`: read-only, the app the permit is for.
    App(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.service`: read-only, the service it reaches.
    GrantService(AccountId, GrantId),
    /// `accounts.clients.<issuer>`.
    Client(Issuer),
    /// `accounts.clients.<issuer>.secret`: the application secret of the client, for the
    /// issuers that need one ([`SECRET_ISSUERS`]).
    ClientSecret(Issuer),
}

/// `word` with its first letter in capitals ("microsoft" -> "Microsoft").
fn sentence_case(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The account a key path names and the rest of the path after it: an id may hold dots, so the
/// account is found by prefix, the longest id first. Every daemon that serves rows of an account
/// finds it here, so that two modules spell one account's rows the same way.
pub fn account_tail<'a, 'i>(
    path: &'a str,
    ids: impl IntoIterator<Item = &'i AccountId>,
) -> Option<(AccountId, &'a str)> {
    let rest = path.strip_prefix("accounts.")?;
    let mut ids: Vec<&AccountId> = ids.into_iter().collect();
    ids.sort_by_key(|id| std::cmp::Reverse(id.as_str().len()));
    ids.into_iter().find_map(|id| {
        let tail = rest.strip_prefix(id.as_str())?.strip_prefix('.')?;
        Some((id.clone(), tail))
    })
}

/// The row after the account's id that says what it spent on hosted models this month.
pub const SPEND_MONTH: &str = "spend.month";

/// The row after the account's id that says the monthly cap on what it may spend.
pub const SPEND_CAP: &str = "spend.cap";

/// The key `path` names, among the accounts of `registry`.
pub fn parse(path: &str, registry: &Registry) -> Option<Key> {
    let rest = path.strip_prefix("accounts.")?;
    if let Some(issuer) = rest.strip_prefix("clients.") {
        return match issuer.strip_suffix(".secret") {
            Some(issuer) => from_word(issuer)
                .filter(|i| SECRET_ISSUERS.contains(i))
                .map(Key::ClientSecret),
            None => from_word(issuer)
                .filter(|i| ISSUERS.contains(i))
                .map(Key::Client),
        };
    }
    let (id, tail) = account_tail(path, registry.accounts.iter().map(|a| &a.id))?;
    if let Some(row) = Row::named(tail) {
        return Some(Key::Account(id, row));
    }
    if tail == "expires" {
        return Some(Key::Expires(id));
    }
    if let Some(n) = tail.strip_prefix("used_by.") {
        return n.parse::<usize>().ok().map(|n| Key::UsedBy(id, n));
    }
    if let Some(kind) = tail.strip_prefix("service.") {
        let kind: CapabilityKind = from_word(kind)?;
        let account = registry.accounts.iter().find(|a| a.id == id)?;
        return switch_kinds(account)
            .contains(&kind)
            .then_some(Key::Service(id, kind));
    }
    let grant = tail.strip_prefix("grant.")?;
    // A permit's own rows end `.since`, `.scope`, `.app` and `.service`; an id may hold dots, so
    // the suffix counts only when what is left names a permit the registry has.
    let held = |text: &str| {
        GrantId::parse(text)
            .ok()
            .filter(|g| registry.grants.iter().any(|held| held.id == *g))
    };
    for (suffix, make) in [
        (".since", Key::Since as fn(AccountId, GrantId) -> Key),
        (".scope", Key::Scope),
        (".app", Key::App),
        (".service", Key::GrantService),
    ] {
        if let Some(found) = grant.strip_suffix(suffix).and_then(held) {
            return Some(make(id, found));
        }
    }
    held(grant).map(|g| Key::Grant(id, g))
}

/// The path of `key`.
pub fn path(key: &Key) -> String {
    match key {
        Key::Account(id, row) => row.path(id),
        Key::Service(id, kind) => format!("accounts.{id}.service.{}", word(kind)),
        Key::Grant(id, grant) => format!("accounts.{id}.grant.{grant}"),
        Key::Since(id, grant) => format!("accounts.{id}.grant.{grant}.since"),
        Key::Scope(id, grant) => format!("accounts.{id}.grant.{grant}.scope"),
        Key::App(id, grant) => format!("accounts.{id}.grant.{grant}.app"),
        Key::GrantService(id, grant) => format!("accounts.{id}.grant.{grant}.service"),
        Key::Expires(id) => format!("accounts.{id}.expires"),
        Key::UsedBy(id, n) => format!("accounts.{id}.used_by.{n}"),
        Key::Client(issuer) => format!("accounts.clients.{}", word(issuer)),
        Key::ClientSecret(issuer) => format!("accounts.clients.{}.secret", word(issuer)),
    }
}

/// A row of this module that is not one every account has.
fn row(
    key: &Key,
    section: &str,
    label: String,
    help: &str,
    kind: KeyKind,
    default: toml::Value,
) -> KeySpec {
    let exposure = match key {
        Key::Client(_) | Key::ClientSecret(_) => Exposure::Advanced,
        _ => Exposure::Basic,
    };
    spec_of(path(key), section, label, help, kind, default, exposure)
}

/// What the `expires` row says: when Google signs the account out, or that it did. Only for an
/// account whose sign-in expires and whose sign-in date is known. The date is the UTC day.
pub fn expiry_text(account: &Account) -> Option<String> {
    let day = account.restriction.expires_at()?.utc_date()?;
    Some(match account.state {
        AccountState::NeedsReauth => format!("Signed out by Google on {day}"),
        _ => format!("Google signs this account out on {day}"),
    })
}

/// The kinds of service of `account` that have a switch: the AI and assistant services it
/// offers, and the ones the person turned off. An account with several models holds one claim
/// per model of the same kind, and has one switch.
fn switch_kinds(account: &Account) -> Vec<CapabilityKind> {
    let mut kinds: Vec<CapabilityKind> = Vec::new();
    for kind in account
        .capabilities
        .iter()
        .filter_map(|claim| match &claim.offer {
            Offer::Present(_) => Some(claim.offer.kind()),
            Offer::Absent {
                kind,
                reason: AbsentReason::TurnedOff,
            } => Some(*kind),
            Offer::Absent { .. } => None,
        })
        .filter(|kind| kind.is_ai() || *kind == CapabilityKind::Agent)
    {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

/// How long a permit holds, as a person reads it.
const SCOPE_WORDS: [(&str, &str); 3] = [
    ("once", "Once"),
    ("always", "Always"),
    ("session", "This session only"),
];

/// A permit's row: "Companion can use Language model" (allowed) or "Mail can't use Language
/// model" (refused), and "... (this session)" for one that lasts one launcher session.
fn grant_label(grant: &Grant, directory: &dyn Directory) -> String {
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
        directory.app_title(&grant.key.app.name),
        grant.key.kind.name()
    )
}

/// What the `since` row of `grant` says: the UTC day it was given, `YYYY-MM-DD`.
pub fn since_text(grant: &Grant) -> String {
    grant.at.utc_date().unwrap_or_default()
}

/// The rows of an AI account or an assistant that an identity has none of: a switch for each
/// AI service, and for each permit a Revoke with its day, how long it lasts, its app and its
/// service.
fn service_keys(
    account: &Account,
    grants: &[Grant],
    section: &str,
    directory: &dyn Directory,
) -> Vec<KeySpec> {
    let id = &account.id;
    let mut keys = Vec::new();
    for kind in switch_kinds(account) {
        keys.push(row(
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
        let readout = |key: Key, label: &str, word: &str, labels: &[(&str, &str)]| {
            readout_of(path(&key), section, label, word, labels)
        };
        keys.push(row(
            &Key::Grant(id.clone(), grant.id.clone()),
            section,
            grant_label(grant, directory),
            "Revoking asks the app again the next time it needs the account.",
            action("Revoke", ActionWeight::Plain),
            off(),
        ));
        keys.push(readout(
            Key::Since(id.clone(), grant.id.clone()),
            "Since",
            &since_text(grant),
            &[],
        ));
        keys.push(readout(
            Key::Scope(id.clone(), grant.id.clone()),
            "Lasts",
            grant.scope.word(),
            &SCOPE_WORDS,
        ));
        let app = grant.key.app.name.as_str();
        keys.push(readout(
            Key::App(id.clone(), grant.id.clone()),
            "App",
            app,
            &[(app, &directory.app_title(&grant.key.app.name))],
        ));
        let service = word(&grant.key.kind);
        keys.push(readout(
            Key::GrantService(id.clone(), grant.id.clone()),
            "Service",
            service,
            &[(service, grant.key.kind.name())],
        ));
    }
    keys
}

fn account_keys(
    account: &Account,
    grants: &[Grant],
    used_by: &[AppName],
    directory: &dyn Directory,
) -> Vec<KeySpec> {
    let section = account.label.0.as_str();
    let id = &account.id;
    let names = names_of(account, directory);
    let mut keys: Vec<KeySpec> = READOUTS
        .iter()
        .filter_map(|row| row.0.spec(account, section, &names))
        .collect();
    if let Some(text) = expiry_text(account) {
        keys.push(row(
            &Key::Expires(id.clone()),
            section,
            "Sign-in".to_owned(),
            "Google ends the sign-ins of an app that is not verified yet after 7 days; signing in again starts another 7.",
            KeyKind::Fixed {
                variant: text.clone(),
            },
            toml::Value::String(text),
        ));
    }
    for (n, app) in used_by.iter().enumerate() {
        let title = directory.app_title(app);
        keys.push(row(
            &Key::UsedBy(id.clone(), n),
            section,
            "Used by".to_owned(),
            "",
            KeyKind::Fixed {
                variant: title.clone(),
            },
            toml::Value::String(title),
        ));
    }
    keys.extend(service_keys(account, grants, section, directory));
    keys.extend(
        ACTIONS
            .iter()
            .filter_map(|row| row.0.spec(account, section, &names)),
    );
    keys
}

/// Whether accountd lists `account`: the identities and Tailscale, the AI services and the
/// programs on this computer, and the assistants. Sync's own switches are syncd's.
pub fn listed(account: &Account, directory: &dyn Directory) -> bool {
    ProviderGroup::ALL.contains(&directory.group_of(account))
}

/// The value a read of `key` answers, or `None` when `registry` holds no such row (an account
/// that is gone, an expiry that does not apply, an app that is not there). The sign-in key rows
/// are the person's clients file's, which the daemon reads.
pub fn read(
    key: &Key,
    registry: &Registry,
    used_by: &UsedBy,
    directory: &dyn Directory,
) -> Option<toml::Value> {
    let account = |id: &AccountId| registry.accounts.iter().find(|a| a.id == *id);
    let held = |grant: &GrantId| registry.grants.iter().find(|g| g.id == *grant);
    match key {
        Key::Account(id, row) => {
            let account = account(id)?;
            Some(row.0.read(account, &names_of(account, directory)))
        }
        Key::Service(id, kind) => {
            let on = account(id)?
                .capabilities
                .iter()
                .any(|c| c.offer.kind() == *kind && matches!(c.offer, Offer::Present(_)));
            Some(toml::Value::String(
                if on { "on" } else { "off" }.to_owned(),
            ))
        }
        Key::Grant(..) => Some(off()),
        Key::Since(_, grant) => held(grant).map(|g| toml::Value::String(since_text(g))),
        Key::Scope(_, grant) => held(grant).map(|g| toml::Value::String(g.scope.word().to_owned())),
        Key::App(_, grant) => {
            held(grant).map(|g| toml::Value::String(g.key.app.name.as_str().to_owned()))
        }
        Key::GrantService(_, grant) => {
            held(grant).map(|g| toml::Value::String(word(&g.key.kind).to_owned()))
        }
        Key::Expires(id) => expiry_text(account(id)?).map(toml::Value::String),
        Key::UsedBy(id, n) => used_by
            .get(id)
            .and_then(|apps| apps.get(*n))
            .map(|app| toml::Value::String(directory.app_title(app))),
        Key::Client(_) | Key::ClientSecret(_) => None,
    }
}

/// What the rows of `account` need to know of its provider.
fn names_of<'a>(account: &'a Account, directory: &'a dyn Directory) -> Names<'a> {
    Names {
        provider_label: directory
            .provider_label(&account.provider)
            .unwrap_or_else(|| account.provider.as_str()),
        group: directory.group_of(account),
    }
}

/// The schema of the module for `registry`; `used_by` says which apps use which identity.
pub fn schema(registry: &Registry, used_by: &UsedBy, directory: &dyn Directory) -> LiveSchema {
    let mut key: Vec<KeySpec> = registry
        .accounts
        .iter()
        .filter(|account| listed(account, directory))
        .flat_map(|account| {
            let apps = used_by.get(&account.id).map(Vec::as_slice).unwrap_or(&[]);
            account_keys(account, &registry.grants, apps, directory)
        })
        .collect();
    key.extend(ISSUERS.iter().flat_map(|issuer| {
        let name = sentence_case(word(issuer));
        let text = || KeyKind::Text;
        let empty = || toml::Value::String(String::new());
        let id = row(
            &Key::Client(*issuer),
            "Sign-in keys",
            format!("{name} sign-in key"),
            client_help(*issuer),
            text(),
            empty(),
        );
        let secret = SECRET_ISSUERS.contains(issuer).then(|| {
            row(
                &Key::ClientSecret(*issuer),
                "Sign-in keys",
                format!("{name} sign-in secret"),
                "The secret Google shows beside the sign-in key. Google asks for it even from an app on your computer; it is not your password.",
                text(),
                empty(),
            )
        });
        std::iter::once(id).chain(secret)
    }));
    LiveSchema { version: 1, key }
}

/// What a sign-in key row says, by issuer: Microsoft's has one built in; Google's is the
/// person's own or nothing, and Google accounts cannot be added without it.
fn client_help(issuer: Issuer) -> &'static str {
    match issuer {
        Issuer::Google => {
            "Google accounts can be added once you paste a sign-in key here: make one for a desktop app in your own Google Cloud project, and paste its secret below. Leave it empty to remove it."
        }
        _ => {
            "A sign-in key from your own Google or Microsoft developer account. Leave it empty to use the one built in."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SIGN_IN, place_slug};
    use ds_settings::schema::{ActionWeight, AgentSetting};
    use porter_core::AuthKind;
    use porter_fake::{mail_account, storage_account};

    /// A directory that knows no provider file and no app name: the id stands for the label and
    /// the sign-in decides the group.
    struct Plain;

    impl Directory for Plain {
        fn provider_label(&self, _: &ProviderId) -> Option<&str> {
            None
        }

        fn group_of(&self, account: &Account) -> ProviderGroup {
            let kinds: Vec<_> = account
                .capabilities
                .iter()
                .map(|c| c.offer.kind())
                .collect();
            ProviderGroup::fallback(account.auth, &kinds)
        }

        fn app_title(&self, app: &AppName) -> String {
            app.as_str().to_owned()
        }
    }

    /// A directory that knows Fastmail, an Internet account provider.
    struct Fastmail;

    impl Directory for Fastmail {
        fn provider_label(&self, provider: &ProviderId) -> Option<&str> {
            (provider.as_str() == "fastmail").then_some("Fastmail")
        }

        fn group_of(&self, _: &Account) -> ProviderGroup {
            ProviderGroup::Internet
        }

        fn app_title(&self, app: &AppName) -> String {
            app.as_str().to_owned()
        }
    }

    fn registry() -> Registry {
        Registry {
            grants: vec![],
            accounts: vec![storage_account(), mail_account()],
            toggles: vec![],
        }
    }

    fn app(name: &str) -> AppName {
        AppName::parse(name).expect("name")
    }

    fn no_apps() -> UsedBy {
        UsedBy::new()
    }

    fn rows(registry: &Registry) -> Vec<KeySpec> {
        schema(registry, &no_apps(), &Plain).key
    }

    #[test]
    fn every_key_is_under_accounts_and_hands_off_and_the_schema_checks() {
        let schema = schema(&registry(), &no_apps(), &Plain);
        assert!(schema.check().is_ok());
        assert!(schema.key.iter().all(|k| k.path.0.starts_with("accounts.")));
        assert!(schema.key.iter().all(|k| k.agent == AgentSetting::HandsOff));
        let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
        for want in [
            "accounts.fake-storage.remove",
            "accounts.fake-storage.reauth",
            "accounts.fake-mail.state",
            "accounts.fake-mail.place",
            "accounts.clients.microsoft",
        ] {
            assert!(paths.contains(&want), "{want} in {paths:?}");
        }
        // An identity has no service switch, permit or sync row.
        for gone in [".service.", ".grant.", ".sync.", ".sign_out"] {
            assert!(
                !paths.iter().any(|p| p.contains(gone)),
                "{gone} in {paths:?}"
            );
        }
    }

    /// The apps that use an identity are listed under it, by the name a person reads, as
    /// read-only rows that parse and print both ways.
    #[test]
    fn the_apps_using_an_identity_are_read_only_rows_under_it() {
        let registry = registry();
        let storage = registry.accounts[0].id.clone();
        let mut used = UsedBy::new();
        used.insert(
            storage.clone(),
            vec![app("org.example.Mail"), app("org.quire.Photos")],
        );
        let schema = schema(&registry, &used, &Plain);
        let used_rows: Vec<(String, String)> = schema
            .key
            .iter()
            .filter(|k| k.path.0.contains(".used_by."))
            .map(|k| match &k.kind {
                KeyKind::Fixed { variant } => (k.path.0.clone(), variant.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            used_rows,
            [
                (
                    "accounts.fake-storage.used_by.0".to_owned(),
                    "org.example.Mail".to_owned()
                ),
                (
                    "accounts.fake-storage.used_by.1".to_owned(),
                    "org.quire.Photos".to_owned()
                ),
            ]
        );
        assert!(schema.check().is_ok());
        let key = Key::UsedBy(storage, 1);
        assert_eq!(
            parse("accounts.fake-storage.used_by.1", &registry),
            Some(key.clone())
        );
        assert_eq!(path(&key), "accounts.fake-storage.used_by.1");
        assert_eq!(parse("accounts.fake-storage.used_by.x", &registry), None);
    }

    #[test]
    fn every_account_says_how_it_signs_in_again_on_a_read_only_row() {
        let way_of = |account: &Account| {
            let rows = account_keys(account, &[], &[], &Plain);
            let row = rows
                .iter()
                .find(|k| k.path.0 == format!("accounts.{}.sign_in", account.id))
                .expect("sign_in row");
            match &row.kind {
                KeyKind::Fixed { variant } => variant.clone(),
                other => panic!("{other:?}"),
            }
        };
        let mut password = mail_account();
        password.auth = AuthKind::Password;
        assert_eq!(way_of(&password), "password");
        let mut oauth = mail_account();
        oauth.auth = AuthKind::OAuthPkce;
        assert_eq!(way_of(&oauth), "browser");
        let registry = registry();
        let id = &registry.accounts[1].id;
        assert_eq!(
            parse(&format!("accounts.{id}.sign_in"), &registry),
            Some(Key::Account(id.clone(), SIGN_IN))
        );
    }

    /// The `Fixed` value and the labels map of the row at `path`.
    fn fixed_row(rows: &[KeySpec], path: &str) -> (String, Vec<(String, String)>) {
        let row = rows
            .iter()
            .find(|k| k.path.0 == path)
            .unwrap_or_else(|| panic!("{path}"));
        let variant = match &row.kind {
            KeyKind::Fixed { variant } => variant.clone(),
            other => panic!("{other:?}"),
        };
        let mut labels: Vec<(String, String)> = row
            .labels
            .0
            .iter()
            .map(|(word, label)| (word.clone(), label.clone()))
            .collect();
        labels.sort();
        (variant, labels)
    }

    #[test]
    fn an_account_says_its_provider_and_its_group_on_two_read_only_rows() {
        let mut account = storage_account();
        account.provider = ProviderId::parse("fastmail").expect("id");
        let rows = account_keys(&account, &[], &[], &Fastmail);
        let id = &account.id;
        assert_eq!(
            fixed_row(&rows, &format!("accounts.{id}.provider")),
            (
                "fastmail".to_owned(),
                vec![("fastmail".to_owned(), "Fastmail".to_owned())]
            )
        );
        let (group, labels) = fixed_row(&rows, &format!("accounts.{id}.group"));
        assert_eq!(group, "internet");
        let mut want: Vec<(String, String)> = [
            ("internet", "Internet Accounts"),
            ("intelligence", "Intelligence"),
            ("agent", "Assistants"),
        ]
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect();
        want.sort();
        assert_eq!(labels, want);
        // No provider file: the id stands for the label and the sign-in decides the group.
        let mut agent = storage_account();
        agent.auth = AuthKind::AgentLogin;
        let rows = account_keys(&agent, &[], &[], &Plain);
        let id = &agent.id;
        let (group, _) = fixed_row(&rows, &format!("accounts.{id}.group"));
        assert_eq!(group, "agent");
        let (provider, labels) = fixed_row(&rows, &format!("accounts.{id}.provider"));
        assert_eq!(labels, vec![(provider.clone(), provider)]);
    }

    #[test]
    fn no_label_or_help_a_person_reads_uses_a_developer_word() {
        const JARGON: [&str; 25] = [
            "acp", "mcp", "oauth", "pkce", "imap", "smtp", "pop", "jmap", "dav", "caldav",
            "carddav", "api", "cli", "token", "endpoint", "relay", "scope", "porter", "accountd",
            "syncd", "inferd", "socket", "localapi", "daemon", "node",
        ];
        let mut registry = registry();
        // Tailscale, signed out (it offers "Sign in") and working, with ids of their own.
        for (id, state) in [
            ("tailscale", AccountState::NeedsLogin),
            ("tailscale-2", AccountState::Ok),
        ] {
            let mut tailscale = storage_account();
            tailscale.id = porter_core::AccountId::parse(id).expect("id");
            tailscale.auth = AuthKind::OwnProgram;
            tailscale.state = state;
            registry.accounts.push(tailscale);
        }
        let mut used = UsedBy::new();
        used.insert(
            registry.accounts[0].id.clone(),
            vec![app("org.example.Mail")],
        );
        let schema = schema(&registry, &used, &Plain);
        for key in &schema.key {
            let mut texts = vec![key.label.0.clone(), key.help.0.clone()];
            texts.extend(key.labels.0.values().cloned());
            for text in texts {
                let lower = text.to_lowercase();
                for word in lower.split(|c: char| !c.is_alphanumeric()) {
                    assert!(!JARGON.contains(&word), "{}: {text:?}", key.path.0);
                }
            }
        }
        let client = schema
            .key
            .iter()
            .find(|k| k.path.0 == "accounts.clients.microsoft")
            .expect("client row");
        assert_eq!(client.label.0, "Microsoft sign-in key");
        assert_eq!(
            client.help.0,
            "A sign-in key from your own Google or Microsoft developer account. Leave it empty to use the one built in."
        );
    }

    /// ux-3: Settings has a Google sign-in key row and its secret, plainly worded, and the
    /// secret row is Google's only.
    #[test]
    fn google_has_a_sign_in_key_row_and_a_secret_row() {
        let schema = schema(&registry(), &no_apps(), &Plain);
        let row = |path: &str| {
            schema
                .key
                .iter()
                .find(|k| k.path.0 == path)
                .map(|k| (k.label.0.clone(), k.help.0.clone(), k.exposure))
        };
        let (label, help, exposure) = row("accounts.clients.google").expect("google key row");
        assert_eq!(label, "Google sign-in key");
        assert!(
            help.starts_with("Google accounts can be added once"),
            "{help}"
        );
        assert_eq!(exposure, Exposure::Advanced);
        let (label, help, _) = row("accounts.clients.google.secret").expect("google secret row");
        assert_eq!(label, "Google sign-in secret");
        assert!(help.contains("not your password"), "{help}");
        assert_eq!(row("accounts.clients.microsoft.secret"), None);
        assert_eq!(
            parse("accounts.clients.google.secret", &registry()),
            Some(Key::ClientSecret(Issuer::Google))
        );
    }

    /// An AI key, a program on this computer and an assistant are listed beside the identities,
    /// and an account elsewhere has its "Sign in again" row while a local runtime has none.
    #[test]
    fn every_group_is_listed_and_a_local_runtime_has_no_sign_in_again() {
        let mut runtime = storage_account();
        runtime.id = porter_core::AccountId::parse("a-runtime").expect("id");
        runtime.auth = AuthKind::LocalRuntime;
        let mut agent = storage_account();
        agent.id = porter_core::AccountId::parse("an-agent").expect("id");
        agent.auth = AuthKind::AgentLogin;
        let registry = Registry {
            accounts: vec![runtime, agent, mail_account()],
            ..registry()
        };
        let rows = rows(&registry);
        let paths: Vec<&str> = rows.iter().map(|k| k.path.0.as_str()).collect();
        for listed in ["accounts.a-runtime.state", "accounts.an-agent.state"] {
            assert!(paths.contains(&listed), "{listed} in {paths:?}");
        }
        assert!(paths.contains(&"accounts.fake-mail.reauth"), "{paths:?}");
        assert!(!paths.contains(&"accounts.a-runtime.reauth"), "{paths:?}");
        let mut runtime = storage_account();
        runtime.auth = AuthKind::LocalRuntime;
        let own = account_keys(&runtime, &[], &[], &Plain);
        assert!(!own.iter().any(|k| k.path.0.ends_with(".reauth")));
    }

    #[test]
    fn a_probed_runtime_is_on_this_computer_and_every_other_account_is_elsewhere() {
        let mut runtime = storage_account();
        runtime.auth = AuthKind::LocalRuntime;
        assert_eq!(place_slug(&runtime), "this_computer");
        assert_eq!(place_slug(&mail_account()), "elsewhere");
    }

    #[test]
    fn a_destructive_action_is_marked_so() {
        let rows = rows(&registry());
        let remove = rows
            .iter()
            .find(|k| k.path.0 == "accounts.fake-mail.remove")
            .expect("remove");
        assert!(matches!(
            &remove.kind,
            KeyKind::Live { action } if action.weight == ActionWeight::Destructive
        ));
    }

    #[test]
    fn paths_parse_back_to_their_keys() {
        let registry = registry();
        for key in &rows(&registry) {
            let parsed = parse(&key.path.0, &registry).unwrap_or_else(|| panic!("{}", key.path.0));
            assert_eq!(path(&parsed), key.path.0);
        }
        for bad in [
            "accounts.nobody.remove",
            "accounts.fake-mail.service.storage",
            "accounts.fake-mail.grant.g1",
            "accounts.fake-mail.sync.files",
            "accounts.clients.openrouter",
            "accounts.clients.microsoft.secret",
            "dock.size",
            "accounts.fake-mail.nonsense",
        ] {
            assert_eq!(parse(bad, &registry), None, "{bad}");
        }
    }

    /// A registry whose language-model account holds one permit of Companion, with a dotted id.
    fn ai_registry() -> Registry {
        use porter_capability::StorageScope;
        use porter_core::consent::Usage;
        use porter_core::{DataClass, GrantKey, SpaceScope};
        let account = porter_fake::llm_account();
        Registry {
            grants: vec![Grant {
                id: GrantId::parse("g.1").expect("id"),
                key: GrantKey {
                    app: porter_core::AppId {
                        name: app("org.quire.Companion"),
                        isolation: porter_core::Isolation::Unsandboxed,
                    },
                    account: account.id.clone(),
                    kind: CapabilityKind::Llm,
                    class: DataClass::Prompt,
                    usage: Usage::Interactive,
                    space: SpaceScope::Any,
                    reach: StorageScope::AppFolder,
                },
                decision: Decision::Allow,
                scope: GrantScope::Always,
                at: porter_ids::UnixSeconds(0),
            }],
            accounts: vec![account],
            toggles: vec![],
        }
    }

    #[test]
    fn every_ai_row_path_reads_back_as_its_key_and_a_dotted_permit_id_is_found() {
        let registry = ai_registry();
        let id = registry.accounts[0].id.clone();
        let grant = GrantId::parse("g.1").expect("id");
        let keys = [
            Key::Service(id.clone(), CapabilityKind::Llm),
            Key::Grant(id.clone(), grant.clone()),
            Key::Since(id.clone(), grant.clone()),
            Key::Scope(id.clone(), grant.clone()),
            Key::App(id.clone(), grant.clone()),
            Key::GrantService(id, grant),
        ];
        for key in keys {
            assert_eq!(parse(&path(&key), &registry), Some(key.clone()), "{key:?}");
        }
        assert_eq!(parse("ai.model.text.fast", &registry), None);
        assert_eq!(parse("accounts.nobody.state", &registry), None);
        let id = &registry.accounts[0].id;
        assert_eq!(parse(&format!("accounts.{id}.grant.g.2"), &registry), None);
    }

    #[test]
    fn the_permit_of_an_app_is_a_revoke_row_with_its_day_and_how_long_it_lasts() {
        let registry = ai_registry();
        let rows = schema(&registry, &no_apps(), &Plain).key;
        let paths: Vec<&str> = rows.iter().map(|r| r.path.0.as_str()).collect();
        let id = &registry.accounts[0].id;
        for suffix in [
            "service.llm",
            "grant.g.1",
            "grant.g.1.since",
            "grant.g.1.scope",
            "grant.g.1.app",
            "grant.g.1.service",
            "remove",
            "label",
        ] {
            let want = format!("accounts.{id}.{suffix}");
            assert!(paths.contains(&want.as_str()), "{want} in {paths:?}");
        }
        let scope = fixed_row(&rows, &format!("accounts.{id}.grant.g.1.scope"));
        assert_eq!(scope.0, "always");
        assert!(
            scope
                .1
                .contains(&("session".to_owned(), "This session only".to_owned()))
        );
        let revoke = rows
            .iter()
            .find(|k| k.path.0 == format!("accounts.{id}.grant.g.1"))
            .expect("revoke");
        assert_eq!(revoke.label.0, "org.quire.Companion can use Language model");
    }

    #[test]
    fn a_permit_names_its_app_and_its_service_in_rows_of_their_own() {
        let registry = ai_registry();
        let rows = schema(&registry, &no_apps(), &Plain).key;
        let id = &registry.accounts[0].id;
        let (app, labels) = fixed_row(&rows, &format!("accounts.{id}.grant.g.1.app"));
        assert_eq!(app, "org.quire.Companion");
        assert_eq!(labels, vec![(app.clone(), app)]);
        let (service, labels) = fixed_row(&rows, &format!("accounts.{id}.grant.g.1.service"));
        assert_eq!(service, word(&CapabilityKind::Llm));
        assert_eq!(
            labels,
            vec![(service, CapabilityKind::Llm.name().to_owned())]
        );
        let read = |key: Key| read(&key, &registry, &no_apps(), &Plain);
        let grant = GrantId::parse("g.1").expect("id");
        assert_eq!(
            read(Key::Service(id.clone(), CapabilityKind::Llm)),
            Some(toml::Value::String("on".into()))
        );
        assert_eq!(
            read(Key::Since(id.clone(), grant.clone())),
            Some(toml::Value::String("1970-01-01".into()))
        );
        assert_eq!(read(Key::Grant(id.clone(), grant)), Some(off()));
    }

    /// inferd serves the spend rows of the accounts accountd lists: both find an account in a
    /// path by the same function, so one account has one spelling in both modules, dotted ids
    /// included.
    #[test]
    fn both_modules_spell_one_accounts_rows_the_same() {
        let registry = ai_registry();
        let ids: Vec<&AccountId> = registry.accounts.iter().map(|a| &a.id).collect();
        let id = ids[0];
        for tail in [SPEND_MONTH, SPEND_CAP] {
            let spelled = format!("accounts.{id}.{tail}");
            assert_eq!(
                account_tail(&spelled, ids.iter().copied()),
                Some((id.clone(), tail))
            );
        }
        // An id with a dot, and a longer id that starts with a shorter one.
        let dotted = AccountId::parse("a.b").expect("id");
        let short = AccountId::parse("a").expect("id");
        let both = [&short, &dotted];
        assert_eq!(
            account_tail("accounts.a.b.spend.cap", both.iter().copied()),
            Some((dotted.clone(), "spend.cap"))
        );
        assert_eq!(
            account_tail("accounts.a.spend.cap", both),
            Some((short.clone(), "spend.cap"))
        );
        // The rows accountd lists for that account are under the same prefix.
        let rows = schema(&registry, &no_apps(), &Plain).key;
        assert!(
            rows.iter()
                .filter(|row| !row.path.0.starts_with("accounts.clients."))
                .all(|row| row.path.0.starts_with(&format!("accounts.{id}.")))
        );
    }
}
