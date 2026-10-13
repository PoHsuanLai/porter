//! The keys of accountd's settings module (PLAN §2.5), as a pure function of the registry: for
//! each identity (the Internet accounts, and Tailscale) its label, place, state, how it signs in,
//! its provider and group (read-outs), the apps using it (`used_by.<n>`, read-outs), "Sign in
//! again" and "Remove account" actions, and one text row per OAuth issuer for a bring-your-own
//! client id. The AI accounts, assistants and sync switches are listed by inferd's and syncd's own
//! modules. Every key is under `accounts.`, so it is never agent-settable (`ds-settings` refuses
//! a schema that says otherwise).

use ds_settings::live::LiveSchema;
use ds_settings::schema::{
    ActionLabel, ActionWeight, AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label,
    LiveAction, Page, Section, WordLabels,
};
use porter_core::sheet::ProviderGroup;
use porter_core::{Account, AccountId, AccountState, AppName, AuthKind};
use porter_families::set::ProviderNames;
use porter_provider::Issuer;
use porter_service::Registry;
use porter_sheets::AppNames;
use std::collections::BTreeMap;

/// The issuers a bring-your-own client id may be set for. The writer changes only the id (and the
/// secret) of a row, so a hand-written row's other lines (Google's `testing`, `byo`) are kept.
pub(crate) const ISSUERS: &[Issuer] = &[Issuer::Microsoft, Issuer::Google];

/// The issuers whose client also needs its application secret (Google's desktop clients): they
/// get a second row, `accounts.clients.<issuer>.secret`.
pub(crate) const SECRET_ISSUERS: &[Issuer] = &[Issuer::Google];

/// The apps that use each identity, by the account it is (`Identities1.RegisterUse`).
pub(crate) type UsedBy = BTreeMap<AccountId, Vec<AppName>>;

/// What a key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// `accounts.<id>.state`.
    State(AccountId),
    /// `accounts.<id>.label`.
    Label(AccountId),
    /// `accounts.<id>.expires`: read-only, only for a sign-in that expires (Google, a client in
    /// testing).
    Expires(AccountId),
    /// `accounts.<id>.place`.
    Place(AccountId),
    /// `accounts.<id>.sign_in`: read-only, how the account is signed in again (porter-core's
    /// `SignInWay` slug: `browser`, `password`, `key`, `agent`, `outside`, `nothing`).
    SignIn(AccountId),
    /// `accounts.<id>.provider`: read-only, the provider the account was made from (the value is
    /// the provider's id, the row's labels give its name).
    Provider(AccountId),
    /// `accounts.<id>.group`: read-only, the part of the Accounts page the account is listed
    /// under (`internet`, `intelligence`, `agent`).
    Group(AccountId),
    /// `accounts.<id>.used_by.<n>`: read-only, the display name of the n-th app (from 0, in name
    /// order) that signed in with this identity.
    UsedBy(AccountId, usize),
    /// `accounts.<id>.reauth`.
    Reauth(AccountId),
    /// `accounts.<id>.remove`.
    Remove(AccountId),
    /// `accounts.clients.<issuer>`.
    Client(Issuer),
    /// `accounts.clients.<issuer>.secret`: the application secret of the client, for the
    /// issuers that need one ([`SECRET_ISSUERS`]).
    ClientSecret(Issuer),
}

fn slug<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `word` with its first letter in capitals ("microsoft" -> "Microsoft").
fn sentence_case(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn from_slug<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// The key `path` names, among the accounts of `registry` (an id may hold dots, so the account
/// is found by prefix, the longest id first).
pub(crate) fn parse(path: &str, registry: &Registry) -> Option<Key> {
    let rest = path.strip_prefix("accounts.")?;
    if let Some(issuer) = rest.strip_prefix("clients.") {
        return match issuer.strip_suffix(".secret") {
            Some(issuer) => from_slug(issuer)
                .filter(|i| SECRET_ISSUERS.contains(i))
                .map(Key::ClientSecret),
            None => from_slug(issuer)
                .filter(|i| ISSUERS.contains(i))
                .map(Key::Client),
        };
    }
    let mut ids: Vec<&AccountId> = registry.accounts.iter().map(|a| &a.id).collect();
    ids.sort_by_key(|id| std::cmp::Reverse(id.as_str().len()));
    ids.into_iter().find_map(|id| {
        let tail = rest.strip_prefix(id.as_str())?.strip_prefix('.')?;
        let id = id.clone();
        match tail {
            "state" => Some(Key::State(id)),
            "label" => Some(Key::Label(id)),
            "expires" => Some(Key::Expires(id)),
            "place" => Some(Key::Place(id)),
            "sign_in" => Some(Key::SignIn(id)),
            "provider" => Some(Key::Provider(id)),
            "group" => Some(Key::Group(id)),
            "reauth" => Some(Key::Reauth(id)),
            "remove" => Some(Key::Remove(id)),
            _ => {
                let n = tail.strip_prefix("used_by.")?;
                n.parse::<usize>().ok().map(|n| Key::UsedBy(id, n))
            }
        }
    })
}

/// The path of `key`.
pub(crate) fn path(key: &Key) -> String {
    match key {
        Key::State(id) => format!("accounts.{id}.state"),
        Key::Label(id) => format!("accounts.{id}.label"),
        Key::Expires(id) => format!("accounts.{id}.expires"),
        Key::Place(id) => format!("accounts.{id}.place"),
        Key::SignIn(id) => format!("accounts.{id}.sign_in"),
        Key::Provider(id) => format!("accounts.{id}.provider"),
        Key::Group(id) => format!("accounts.{id}.group"),
        Key::UsedBy(id, n) => format!("accounts.{id}.used_by.{n}"),
        Key::Reauth(id) => format!("accounts.{id}.reauth"),
        Key::Remove(id) => format!("accounts.{id}.remove"),
        Key::Client(issuer) => format!("accounts.clients.{}", slug(issuer)),
        Key::ClientSecret(issuer) => format!("accounts.clients.{}.secret", slug(issuer)),
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
        path: KeyPath(path(key)),
        kind,
        default,
        label: Label(label),
        help: Help(help.to_owned()),
        page: Page::Accounts,
        section: Section(section.to_owned()),
        exposure: match key {
            Key::Client(_) | Key::ClientSecret(_) => Exposure::Advanced,
            _ => Exposure::Basic,
        },
        labels: Default::default(),
        unavailable: Default::default(),
        groups: Default::default(),
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

/// What the `expires` row says: when Google signs the account out, or that it did. Only for an
/// account whose sign-in expires and whose sign-in date is known. The date is the UTC day.
pub(crate) fn expiry_text(account: &Account) -> Option<String> {
    let day = civil_date(account.restriction.expires_at()?);
    Some(match account.state {
        AccountState::NeedsReauth => format!("Signed out by Google on {day}"),
        _ => format!("Google signs this account out on {day}"),
    })
}

/// `YYYY-MM-DD` of the UTC day `at` falls in (Howard Hinnant's civil-from-days).
fn civil_date(at: porter_core::UnixSeconds) -> String {
    let z = at.0.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `spec` with the words of its enum key labelled for a person.
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

fn account_keys(
    account: &Account,
    used_by: &[AppName],
    names: &AppNames,
    providers: &ProviderNames,
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
    let mut keys = vec![
        spec(
            &Key::Label(id.clone()),
            section,
            "Account".to_owned(),
            "",
            KeyKind::Fixed {
                variant: account.label.0.clone(),
            },
            toml::Value::String(account.label.0.clone()),
        ),
        spec(
            &Key::Place(id.clone()),
            section,
            "Where".to_owned(),
            "",
            KeyKind::Fixed {
                variant: place_slug(account).to_owned(),
            },
            toml::Value::String(place_slug(account).to_owned()),
        ),
        spec(
            &Key::State(id.clone()),
            section,
            "Status".to_owned(),
            "",
            KeyKind::Fixed {
                variant: crate::account::state_slug(account.state).to_owned(),
            },
            toml::Value::String(crate::account::state_slug(account.state).to_owned()),
        ),
        spec(
            &Key::SignIn(id.clone()),
            section,
            "Signs in".to_owned(),
            "",
            KeyKind::Fixed {
                variant: account.auth.sign_in_way().slug().to_owned(),
            },
            toml::Value::String(account.auth.sign_in_way().slug().to_owned()),
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
    if let Some(text) = expiry_text(account) {
        keys.push(spec(
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
        let title = names.title_of(app).0;
        keys.push(spec(
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
    keys.extend(match account.auth {
        // Tailscale signed out: "Sign in" hands the person Tailscale's own page; porter takes no
        // password or key of Tailscale's.
        AuthKind::OwnProgram if account.state == AccountState::NeedsLogin => Some(spec(
            &Key::Reauth(id.clone()),
            section,
            "Sign in".to_owned(),
            "Opens Tailscale's own sign-in page. Your Tailscale password is never seen here.",
            action("Sign in", ActionWeight::Plain),
            off(),
        )),
        // Tailscale keeps its own sign-in, and so is not signed out from here.
        AuthKind::OwnProgram => None,
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
    let remove_help = match account.auth {
        // Nothing of Tailscale's is stored here, and nothing of Tailscale's is touched.
        AuthKind::OwnProgram => {
            "Takes this account out of this computer's accounts. It does not sign Tailscale out."
        }
        _ => "Deletes the account's stored sign-in from this computer.",
    };
    keys.push(spec(
        &Key::Remove(id.clone()),
        section,
        "Remove account".to_owned(),
        remove_help,
        action("Remove account", ActionWeight::Destructive),
        off(),
    ));
    keys
}

/// Where an account lives, as the `place` row says it: a program on this computer (a probed
/// runtime, which signs in with nothing) or anywhere else. Settings lists the first kind under
/// "On this computer".
pub(crate) fn place_slug(account: &Account) -> &'static str {
    match account.auth {
        AuthKind::LocalRuntime => "this_computer",
        _ => "elsewhere",
    }
}

/// Whether accountd lists `account`: the identities and Tailscale (the Internet group). The AI
/// accounts and assistants are listed by inferd's module.
pub(crate) fn listed(account: &Account, providers: &ProviderNames) -> bool {
    providers.group_of(account) == ProviderGroup::Internet
}

/// The schema of the module for `registry`; `used_by` says which apps use which identity.
pub(crate) fn schema(
    registry: &Registry,
    used_by: &UsedBy,
    names: &AppNames,
    providers: &ProviderNames,
) -> LiveSchema {
    let mut key: Vec<KeySpec> = registry
        .accounts
        .iter()
        .filter(|account| listed(account, providers))
        .flat_map(|account| {
            let apps = used_by.get(&account.id).map(Vec::as_slice).unwrap_or(&[]);
            account_keys(account, apps, names, providers)
        })
        .collect();
    key.extend(ISSUERS.iter().flat_map(|issuer| {
        let name = sentence_case(&slug(issuer));
        let text = || KeyKind::Text;
        let empty = || toml::Value::String(String::new());
        let id = spec(
            &Key::Client(*issuer),
            "Sign-in keys",
            format!("{name} sign-in key"),
            client_help(*issuer),
            text(),
            empty(),
        );
        let secret = SECRET_ISSUERS.contains(issuer).then(|| {
            spec(
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
    use porter_fake::{mail_account, storage_account};

    #[test]
    fn the_utc_day_of_a_unix_time() {
        let cases = [
            (0, "1970-01-01"),
            (86_399, "1970-01-01"),
            (86_400, "1970-01-02"),
            (951_782_400, "2000-02-29"),
            (1_790_000_000, "2026-09-21"),
            (-86_400, "1969-12-31"),
        ];
        for (at, want) in cases {
            assert_eq!(civil_date(porter_core::UnixSeconds(at)), want, "{at}");
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
        schema(
            registry,
            &no_apps(),
            &AppNames::default(),
            &ProviderNames::default(),
        )
        .key
    }

    #[test]
    fn every_key_is_under_accounts_and_hands_off_and_the_schema_checks() {
        let schema = schema(
            &registry(),
            &no_apps(),
            &AppNames::default(),
            &ProviderNames::default(),
        );
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
        let schema = schema(
            &registry,
            &used,
            &AppNames::default(),
            &ProviderNames::default(),
        );
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
            let rows = account_keys(
                account,
                &[],
                &AppNames::default(),
                &ProviderNames::default(),
            );
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
            Some(Key::SignIn(id.clone()))
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
        account.provider = porter_core::ProviderId::parse("fastmail").expect("id");
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/fastmail.toml"
        ))
        .expect("fastmail ships");
        let known =
            ProviderNames::from_specs(&[porter_provider::parse_provider(&text).expect("spec")]);
        let rows = account_keys(&account, &[], &AppNames::default(), &known);
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
        let rows = account_keys(&agent, &[], &AppNames::default(), &ProviderNames::default());
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
        let schema = schema(
            &registry,
            &used,
            &AppNames::default(),
            &ProviderNames::default(),
        );
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
        let schema = schema(
            &registry(),
            &no_apps(),
            &AppNames::default(),
            &ProviderNames::default(),
        );
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
        let own = account_keys(
            &runtime,
            &[],
            &AppNames::default(),
            &ProviderNames::default(),
        );
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
