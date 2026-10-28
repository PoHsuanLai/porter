//! The keys of accountd's settings module (PLAN §2.5), as a pure function of the registry: for
//! each identity (the Internet accounts, and Tailscale) the rows every account has
//! ([`crate::ROWS`]), the expiry of a sign-in that expires, the apps using it (`used_by.<n>`,
//! read-outs), and one text row per OAuth issuer for a bring-your-own client id. The AI
//! accounts, assistants and sync switches are listed by inferd's and syncd's own modules. Every
//! key is under `accounts.`, so it is never agent-settable (`ds-settings` refuses a schema that
//! says otherwise).

use crate::{ACTIONS, Names, READOUTS, Row, spec as spec_of};
use ds_settings::live::LiveSchema;
use ds_settings::schema::{Exposure, KeyKind, KeySpec};
use porter_core::{Account, AccountId, AccountState, AppName, ProviderId};
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

/// The key `path` names, among the accounts of `registry` (an id may hold dots, so the account
/// is found by prefix, the longest id first).
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
    let mut ids: Vec<&AccountId> = registry.accounts.iter().map(|a| &a.id).collect();
    ids.sort_by_key(|id| std::cmp::Reverse(id.as_str().len()));
    ids.into_iter().find_map(|id| {
        let tail = rest.strip_prefix(id.as_str())?.strip_prefix('.')?;
        let id = id.clone();
        if let Some(row) = Row::named(tail) {
            return Some(Key::Account(id, row));
        }
        match tail {
            "expires" => Some(Key::Expires(id)),
            _ => {
                let n = tail.strip_prefix("used_by.")?;
                n.parse::<usize>().ok().map(|n| Key::UsedBy(id, n))
            }
        }
    })
}

/// The path of `key`.
pub fn path(key: &Key) -> String {
    match key {
        Key::Account(id, row) => row.path(id),
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

fn account_keys(account: &Account, used_by: &[AppName], directory: &dyn Directory) -> Vec<KeySpec> {
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
    keys.extend(
        ACTIONS
            .iter()
            .filter_map(|row| row.0.spec(account, section, &names)),
    );
    keys
}

/// Whether accountd lists `account`: the identities and Tailscale (the Internet group). The AI
/// accounts and assistants are listed by inferd's module.
pub fn listed(account: &Account, directory: &dyn Directory) -> bool {
    directory.group_of(account) == ProviderGroup::Internet
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
    match key {
        Key::Account(id, row) => {
            let account = account(id)?;
            Some(row.0.read(account, &names_of(account, directory)))
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
            account_keys(account, apps, directory)
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
        // Nothing of the old model is listed: no service switch, grant or sync row.
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
            let rows = account_keys(account, &[], &Plain);
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
        let rows = account_keys(&account, &[], &Fastmail);
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
        let rows = account_keys(&agent, &[], &Plain);
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

    /// An AI key, a program on this computer and an assistant are not identities: accountd lists
    /// none of them (inferd's module does), and an account elsewhere has its "Sign in again" row.
    #[test]
    fn only_identities_and_tailscale_are_listed_and_a_local_runtime_has_no_sign_in_again() {
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
        assert!(
            !paths
                .iter()
                .any(|p| p.contains("a-runtime") || p.contains("an-agent")),
            "{paths:?}"
        );
        assert!(paths.contains(&"accounts.fake-mail.reauth"), "{paths:?}");
        let mut runtime = storage_account();
        runtime.auth = AuthKind::LocalRuntime;
        let own = account_keys(&runtime, &[], &Plain);
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
}
