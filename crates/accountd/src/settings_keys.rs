//! The keys of accountd's settings module (PLAN §2.5), as a pure function of the registry: one
//! row per account for its state and label (read-outs), one toggle per service kind, one
//! Revoke per grant, a sync switch per class syncd can keep of the account, "Sign in again" and
//! "Remove account" actions, and one text row per OAuth
//! issuer for a bring-your-own client id. Every key is under `accounts.`, so it is never
//! agent-settable (`ds-settings` refuses a schema that says otherwise).

use crate::app_names::AppNames;
use crate::provider_names::ProviderNames;
use ds_settings::live::LiveSchema;
use ds_settings::schema::{
    ActionLabel, ActionWeight, AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label,
    LiveAction, Page, Section, WordLabels,
};
use porter_core::consent::{Decision, Grant, GrantScope};
use porter_core::sheet::ProviderGroup;
use porter_core::{Account, AccountId, AccountState, AuthKind, CapabilityKind, GrantId, Offer};
use porter_provider::Issuer;
use porter_service::{Registry, SyncClass, sync_offers};

/// The issuers a bring-your-own client id may be set for. The writer changes only the id (and the
/// secret) of a row, so a hand-written row's other lines (Google's `testing`, `byo`) are kept.
pub(crate) const ISSUERS: &[Issuer] = &[Issuer::Microsoft, Issuer::Google];

/// The issuers whose client also needs its application secret (Google's desktop clients): they
/// get a second row, `accounts.clients.<issuer>.secret`.
pub(crate) const SECRET_ISSUERS: &[Issuer] = &[Issuer::Google];

/// What a key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// `accounts.<id>.service.<kind>`.
    Service(AccountId, CapabilityKind),
    /// `accounts.<id>.sync.files` and `.sync.photos`: whether syncd keeps that on this computer.
    Sync(AccountId, SyncClass),
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
    /// `accounts.<id>.grant.<grant>`.
    Grant(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.since`: read-only, the UTC day (`YYYY-MM-DD`) the grant was
    /// given.
    Since(AccountId, GrantId),
    /// `accounts.<id>.grant.<grant>.scope`: read-only, how long the grant holds (`once`,
    /// `always`, `session`).
    Scope(AccountId, GrantId),
    /// `accounts.<id>.reauth`.
    Reauth(AccountId),
    /// `accounts.<id>.sign_out`: for an agent that signs itself in, forgets that it was signed in.
    SignOut(AccountId),
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
            "sign_out" => Some(Key::SignOut(id)),
            "remove" => Some(Key::Remove(id)),
            _ => {
                if let Some(kind) = tail.strip_prefix("service.") {
                    return from_slug(kind).map(|k| Key::Service(id, k));
                }
                if let Some(what) = tail.strip_prefix("sync.") {
                    return SyncClass::ALL
                        .into_iter()
                        .find(|c| c.slug() == what)
                        .map(|c| Key::Sync(id, c));
                }
                let grant = tail.strip_prefix("grant.")?;
                // A grant's own rows end `.since` and `.scope`; a grant id may hold dots, so
                // the suffix counts only when what is left names a grant the registry has.
                for (suffix, make) in [
                    (".since", Key::Since as fn(AccountId, GrantId) -> Key),
                    (".scope", Key::Scope),
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
        Key::Sync(id, class) => format!("accounts.{id}.sync.{}", class.slug()),
        Key::State(id) => format!("accounts.{id}.state"),
        Key::Label(id) => format!("accounts.{id}.label"),
        Key::Expires(id) => format!("accounts.{id}.expires"),
        Key::Place(id) => format!("accounts.{id}.place"),
        Key::SignIn(id) => format!("accounts.{id}.sign_in"),
        Key::Provider(id) => format!("accounts.{id}.provider"),
        Key::Group(id) => format!("accounts.{id}.group"),
        Key::Grant(id, grant) => format!("accounts.{id}.grant.{grant}"),
        Key::Since(id, grant) => format!("accounts.{id}.grant.{grant}.since"),
        Key::Scope(id, grant) => format!("accounts.{id}.grant.{grant}.scope"),
        Key::Reauth(id) => format!("accounts.{id}.reauth"),
        Key::SignOut(id) => format!("accounts.{id}.sign_out"),
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

/// A service as a person reads it: the Services switch's label, and the noun in a grant's row
/// (porter-core's `CapabilityKind::display_name`, which sill's account sheet reads too).
pub(crate) fn service_label(kind: &CapabilityKind) -> &'static str {
    kind.display_name()
}

/// A grant's row: "Sync can use Files" (allowed) or "Mail can't use Contacts" (refused), and
/// "Claude Code can use Language model (this session)" for a grant that lasts one launcher session.
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
        service_label(&grant.key.kind)
    )
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

/// What the `since` row of `grant` says: the UTC day it was given, `YYYY-MM-DD`.
pub(crate) fn since_text(grant: &Grant) -> String {
    civil_date(grant.at)
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

/// What the `scope` row of a grant says: how long the answer holds.
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
            service_label(&kind).to_owned(),
            "Whether apps may use this service of the account.",
            KeyKind::Toggle {
                variants: ["on".to_owned(), "off".to_owned()],
            },
            toml::Value::String("on".to_owned()),
        ));
    }
    for class in sync_offers(account) {
        let (label, help) = match class {
            SyncClass::Files => (
                "Keep this account's files on this computer",
                "Lets the sync service keep a copy of this account's app folder here.",
            ),
            // Google Photos is not a backup of a library: it is an upload folder, and what the
            // person picks in Google's own window. The row says so.
            SyncClass::Photos
                if account
                    .endpoints
                    .iter()
                    .any(|e| e.family == porter_core::Family::GooglePhotosUpload) =>
            {
                (
                    "Upload new photos from a folder on this computer to Google Photos",
                    "Sends each new photo you put in this account's upload folder, a folder on this computer, to an album made for it in Google Photos. It never reads or deletes anything else in your Google Photos.",
                )
            }
            SyncClass::Photos => (
                "Back up photos",
                "Lets the sync service keep this account's photos here and back them up.",
            ),
        };
        keys.push(spec(
            &Key::Sync(id.clone(), class),
            section,
            label.to_owned(),
            help,
            KeyKind::Toggle {
                variants: ["on".to_owned(), "off".to_owned()],
            },
            toml::Value::String("off".to_owned()),
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
    }
    keys.extend(match account.auth {
        // An agent that is not signed in is offered "Sign in": its launcher asks the agent to
        // sign itself in, as an app's Reauthenticate does.
        AuthKind::AgentLogin if account.state == AccountState::NeedsLogin => Some(spec(
            &Key::Reauth(id.clone()),
            section,
            "Sign in".to_owned(),
            "Asks the assistant to sign itself in, which needs its app to be running.",
            action("Sign in", ActionWeight::Plain),
            off(),
        )),
        // An agent signs itself in, inside the agent: porter holds only whether it said so, and
        // signing out forgets that. The agent's own login files are the agent's.
        AuthKind::AgentLogin => Some(spec(
            &Key::SignOut(id.clone()),
            section,
            "Sign out".to_owned(),
            "Asks the assistant to sign itself out when it is running; otherwise it only forgets here that it was signed in, and the assistant's own login is not touched.",
            action("Sign out", ActionWeight::Plain),
            off(),
        )),
        // Tailscale signed out: "Sign in" hands the person Tailscale's own page; porter takes no
        // password or key of Tailscale's.
        AuthKind::OwnProgram if account.state == AccountState::NeedsLogin => Some(spec(
            &Key::Reauth(id.clone()),
            section,
            "Sign in".to_owned(),
            "Opens Tailscale's own sign-in page. porter never sees your Tailscale password.",
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
        _ => "Deletes the account's stored credentials and every permission given for it.",
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

/// The schema of the module for `registry`.
pub(crate) fn schema(
    registry: &Registry,
    names: &AppNames,
    providers: &ProviderNames,
) -> LiveSchema {
    let mut key: Vec<KeySpec> = registry
        .accounts
        .iter()
        .flat_map(|account| account_keys(account, &registry.grants, names, providers))
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
    use porter_core::consent::{GrantKey, GrantScope, Usage};
    use porter_core::{AppId, AppName, DataClass, Isolation, SpaceScope, UnixSeconds};
    use porter_fake::{mail_account, storage_account};

    fn registry() -> Registry {
        let storage = storage_account();
        Registry {
            grants: vec![Grant {
                id: GrantId::parse("g1").expect("id"),
                key: GrantKey {
                    app: AppId {
                        name: AppName::parse("org.quire.Photos").expect("name"),
                        isolation: Isolation::Flatpak,
                    },
                    account: storage.id.clone(),
                    kind: CapabilityKind::Storage,
                    class: DataClass::Files,
                    usage: Usage::Interactive,
                    space: SpaceScope::Any,
                },
                decision: Decision::Allow,
                scope: GrantScope::Always,
                at: UnixSeconds(1),
            }],
            accounts: vec![storage, mail_account(), graph()],
            toggles: vec![],
        }
    }

    /// A storage account at a Graph server whose uploads resume.
    fn graph() -> Account {
        let mut account = storage_account();
        account.id = AccountId::parse("fake-graph").expect("id");
        account.endpoints.push(porter_core::ServiceEndpoint {
            family: porter_core::Family::Graph,
            url: porter_core::EndpointUrl::parse("https://graph.invalid").expect("url"),
            tls: porter_core::Tls::Implicit,
            login: porter_core::LoginName("ada".into()),
        });
        for claim in &mut account.capabilities {
            if let Offer::Present(porter_core::Capability::Storage(cap)) = &mut claim.offer {
                cap.chunked_upload = porter_core::capability::Offered::Present;
            }
        }
        account
    }

    #[test]
    fn only_an_account_syncd_can_mirror_has_the_sync_rows_plainly_worded_and_off_by_default() {
        let schema = schema(&registry(), &AppNames::default(), &ProviderNames::default());
        let sync: Vec<_> = schema
            .key
            .iter()
            .filter(|k| k.path.0.contains(".sync."))
            .collect();
        let shown: Vec<(&str, &str)> = sync
            .iter()
            .map(|k| (k.path.0.as_str(), k.label.0.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "accounts.fake-graph.sync.files",
                    "Keep this account's files on this computer"
                ),
                ("accounts.fake-graph.sync.photos", "Back up photos"),
            ]
        );
        for row in sync {
            assert!(matches!(row.kind, KeyKind::Toggle { .. }));
            assert_eq!(row.default, toml::Value::String("off".into()));
            assert_eq!(row.agent, AgentSetting::HandsOff);
        }
    }

    #[test]
    fn a_google_accounts_photos_row_says_it_uploads_a_folder_and_a_microsoft_one_keeps_its_words() {
        use porter_core::capability::{Albums, LibraryRead, Offered, PhotosCap};
        use porter_core::{Capability, Claim, EndpointUrl, Family, LoginName, Offer};
        use porter_core::{Provenance, ServiceEndpoint, Subject, Tls};
        let mut google = storage_account();
        google.id = porter_core::AccountId::parse("fake-google").expect("id");
        google.endpoints = vec![ServiceEndpoint {
            family: Family::GooglePhotosUpload,
            url: EndpointUrl::parse("https://photos.invalid/v1").expect("url"),
            tls: Tls::Implicit,
            login: LoginName("ada@gmail.invalid".into()),
        }];
        google.capabilities = vec![Claim {
            subject: Subject::Account,
            offer: Offer::Present(Capability::Photos(PhotosCap {
                library_read: LibraryRead::PickerOnly,
                upload: Offered::Present,
                albums: Albums::AppCreated,
                video: Offered::Present,
                delta: porter_core::capability::Delta::None,
            })),
            provenance: Provenance::Declared,
        }];
        let row = |account: &Account| {
            account_keys(
                account,
                &[],
                &AppNames::default(),
                &ProviderNames::default(),
            )
            .into_iter()
            .find(|k| k.path.0.ends_with(".sync.photos"))
            .map(|k| (k.label.0, k.help.0))
            .expect("a photos row")
        };
        let (label, help) = row(&google);
        assert_eq!(
            label,
            "Upload new photos from a folder on this computer to Google Photos"
        );
        assert!(
            help.contains("upload folder") && help.contains("never reads or deletes"),
            "{help}"
        );
        assert_eq!(row(&graph()).0, "Back up photos");
    }

    fn grant_row(decision: Decision, app: &str, kind: CapabilityKind, names: &AppNames) -> String {
        let mut grants = registry().grants;
        grants[0].decision = decision;
        grants[0].key.app.name = AppName::parse(app).expect("name");
        grants[0].key.kind = kind;
        let rows = account_keys(
            &storage_account(),
            &grants,
            names,
            &ProviderNames::default(),
        );
        let row = rows
            .iter()
            .find(|k| k.path.0 == "accounts.fake-storage.grant.g1")
            .expect("grant row");
        assert_eq!(
            row.help.0,
            "Revoking asks the app again the next time it needs the account."
        );
        row.label.0.clone()
    }

    #[test]
    fn a_grant_row_says_in_a_sentence_what_the_app_can_or_cannot_use() {
        let dirs = std::env::temp_dir().join(format!("grant-label-{}", std::process::id()));
        std::fs::create_dir_all(&dirs).expect("scratch");
        std::fs::write(
            dirs.join("org.example.Mail.desktop"),
            "[Desktop Entry]\nName=Mail\n",
        )
        .expect("entry");
        let names = AppNames::new(
            porter_dbus::CallerTable {
                callers: vec![porter_dbus::CallerRow {
                    app: AppName::parse("org.quire.Sync").expect("name"),
                    unit: None,
                    role: porter_dbus::CallerRole::PorterDaemon,
                    name: Some(porter_dbus::AppTitle("Sync".into())),
                }],
            },
            vec![dirs.clone()],
        );
        let storage = CapabilityKind::Storage;
        assert_eq!(
            grant_row(Decision::Allow, "org.quire.Sync", storage, &names),
            "Sync can use Files"
        );
        assert_eq!(
            grant_row(
                Decision::Deny,
                "org.example.Mail",
                CapabilityKind::Contacts,
                &names
            ),
            "Mail can't use Contacts"
        );
        assert_eq!(
            grant_row(Decision::Allow, "org.example.Ghost", storage, &names),
            "org.example.Ghost can use Files"
        );
        let _ = std::fs::remove_dir_all(dirs);
    }

    #[test]
    fn the_service_switch_and_the_grant_row_call_a_service_the_same_thing() {
        let rows = account_keys(
            &storage_account(),
            &[],
            &AppNames::default(),
            &ProviderNames::default(),
        );
        let switch = rows
            .iter()
            .find(|k| k.path.0 == "accounts.fake-storage.service.storage")
            .expect("switch");
        assert_eq!(switch.label.0, service_label(&CapabilityKind::Storage));
        assert_eq!(switch.label.0, "Files");
    }

    #[test]
    fn every_kind_of_service_is_called_by_the_words_sills_account_sheet_uses() {
        use CapabilityKind::*;
        let table = [
            (Identity, "Account details"),
            (Mail, "Mail"),
            (Calendar, "Calendar"),
            (Contacts, "Contacts"),
            (Tasks, "Tasks"),
            (Notes, "Notes"),
            (Storage, "Files"),
            (Photos, "Photos"),
            (Llm, "Language model"),
            (Embeddings, "Search by meaning"),
            (Speech, "Speech"),
            (ImageGen, "Image generation"),
            (Rerank, "Result ranking"),
            (ComputerUse, "Operating windows"),
            (KeyValue, "Small synced items"),
            (Push, "Notifications"),
            (Agent, "Assistant"),
        ];
        for (kind, words) in table {
            assert_eq!(service_label(&kind), words, "{kind:?}");
        }
    }

    #[test]
    fn every_key_is_under_accounts_and_hands_off_and_the_schema_checks() {
        let schema = schema(&registry(), &AppNames::default(), &ProviderNames::default());
        assert!(schema.check().is_ok());
        assert!(schema.key.iter().all(|k| k.path.0.starts_with("accounts.")));
        assert!(schema.key.iter().all(|k| k.agent == AgentSetting::HandsOff));
        let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
        for want in [
            "accounts.fake-storage.service.storage",
            "accounts.fake-storage.service.calendar",
            "accounts.fake-storage.grant.g1",
            "accounts.fake-storage.remove",
            "accounts.fake-storage.reauth",
            "accounts.fake-mail.state",
            "accounts.fake-mail.place",
            "accounts.clients.microsoft",
        ] {
            assert!(paths.contains(&want), "{want} in {paths:?}");
        }
    }

    #[test]
    fn a_service_claimed_once_per_model_is_one_switch() {
        let mut runtime = storage_account();
        runtime.capabilities.push(runtime.capabilities[0].clone());
        let rows = account_keys(
            &runtime,
            &[],
            &AppNames::default(),
            &ProviderNames::default(),
        );
        let paths: Vec<&str> = rows.iter().map(|k| k.path.0.as_str()).collect();
        let mut unique = paths.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(paths.len(), unique.len(), "{paths:?}");
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
        let mut agent = storage_account();
        agent.auth = AuthKind::AgentLogin;
        assert_eq!(way_of(&agent), "agent");
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
    fn a_grant_says_the_day_it_was_given_and_how_long_it_holds_on_two_read_only_rows() {
        let registry = registry();
        let account = &registry.accounts[0];
        let mut grants = registry.grants.clone();
        grants[0].at = UnixSeconds(951_782_400);
        let rows = account_keys(
            account,
            &grants,
            &AppNames::default(),
            &ProviderNames::default(),
        );
        let base = format!("accounts.{}.grant.g1", account.id);
        assert_eq!(
            fixed_row(&rows, &format!("{base}.since")),
            ("2000-02-29".to_owned(), vec![])
        );
        let (scope, labels) = fixed_row(&rows, &format!("{base}.scope"));
        assert_eq!(scope, "always");
        assert_eq!(
            labels,
            vec![
                ("always".to_owned(), "Always".to_owned()),
                ("once".to_owned(), "Once".to_owned()),
                ("session".to_owned(), "This session only".to_owned()),
            ]
        );
        // The grant's own row is unchanged and the new rows parse and print both ways.
        assert!(rows.iter().any(|k| k.path.0 == base));
        let id = account.id.clone();
        let g1 = GrantId::parse("g1").expect("id");
        for (suffix, key) in [
            ("since", Key::Since(id.clone(), g1.clone())),
            ("scope", Key::Scope(id.clone(), g1.clone())),
        ] {
            let text = format!("{base}.{suffix}");
            assert_eq!(parse(&text, &registry), Some(key.clone()));
            assert_eq!(path(&key), text);
        }
        assert_eq!(parse(&base, &registry), Some(Key::Grant(id, g1)));
    }

    #[test]
    fn no_label_or_help_a_person_reads_uses_a_developer_word() {
        const JARGON: [&str; 21] = [
            "acp", "mcp", "oauth", "pkce", "imap", "smtp", "pop", "jmap", "dav", "caldav",
            "carddav", "api", "cli", "token", "endpoint", "relay", "scope", "porter", "accountd",
            "syncd", "inferd",
        ];
        let mut agent = storage_account();
        agent.auth = AuthKind::AgentLogin;
        let mut registry = registry();
        registry.accounts.push(agent);
        let schema = schema(&registry, &AppNames::default(), &ProviderNames::default());
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
        let schema = schema(&registry(), &AppNames::default(), &ProviderNames::default());
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

    /// ux-9: an assistant that is not signed in is offered "Sign in" (the reauth row), not
    /// "Sign out"; a signed-in one is offered "Sign out" only.
    #[test]
    fn an_assistant_is_offered_sign_in_when_it_needs_one_and_sign_out_when_signed_in() {
        let rows = |state: AccountState| {
            let mut agent = storage_account();
            agent.auth = AuthKind::AgentLogin;
            agent.state = state;
            let registry = Registry {
                accounts: vec![agent],
                ..registry()
            };
            schema(&registry, &AppNames::default(), &ProviderNames::default())
                .key
                .into_iter()
                .filter(|k| matches!(k.kind, KeyKind::Live { .. }))
                .map(|k| (k.path.0, k.label.0))
                .filter(|(path, _)| path.ends_with(".reauth") || path.ends_with(".sign_out"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(AccountState::NeedsLogin),
            [(
                "accounts.fake-storage.reauth".to_owned(),
                "Sign in".to_owned()
            )]
        );
        assert_eq!(
            rows(AccountState::Ok),
            [(
                "accounts.fake-storage.sign_out".to_owned(),
                "Sign out".to_owned()
            )]
        );
    }

    /// ux-8: a program on this computer has no "Sign in again" row; an account elsewhere has.
    #[test]
    fn a_local_runtime_is_not_offered_sign_in_again() {
        let mut runtime = storage_account();
        runtime.auth = AuthKind::LocalRuntime;
        let registry = Registry {
            accounts: vec![runtime, mail_account()],
            ..registry()
        };
        let schema = schema(&registry, &AppNames::default(), &ProviderNames::default());
        let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
        assert!(
            !paths.contains(&"accounts.fake-storage.reauth"),
            "{paths:?}"
        );
        assert!(paths.contains(&"accounts.fake-storage.remove"), "{paths:?}");
        assert!(paths.contains(&"accounts.fake-mail.reauth"), "{paths:?}");
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
        let schema = schema(&registry(), &AppNames::default(), &ProviderNames::default());
        let remove = schema
            .key
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
        for key in &schema(&registry, &AppNames::default(), &ProviderNames::default()).key {
            let parsed = parse(&key.path.0, &registry).unwrap_or_else(|| panic!("{}", key.path.0));
            assert_eq!(path(&parsed), key.path.0);
        }
        for bad in [
            "accounts.nobody.remove",
            "accounts.fake-mail.service.teleport",
            "accounts.clients.openrouter",
            "accounts.clients.microsoft.secret",
            "dock.size",
            "accounts.fake-mail.nonsense",
        ] {
            assert_eq!(parse(bad, &registry), None, "{bad}");
        }
    }
}
