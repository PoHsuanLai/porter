//! The keys of accountd's settings module (PLAN §2.5), as a pure function of the registry: one
//! row per account for its state and label (read-outs), one toggle per service kind, one
//! Revoke per grant, "Sign in again" and "Remove account" actions, and one text row per OAuth
//! issuer for a bring-your-own client id. Every key is under `accounts.`, so it is never
//! agent-settable (`ds-settings` refuses a schema that says otherwise).

use ds_settings::live::LiveSchema;
use ds_settings::schema::{
    ActionLabel, ActionWeight, AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label,
    LiveAction, Page, Section,
};
use porter_core::consent::{Decision, Grant};
use porter_core::{Account, AccountId, CapabilityKind, GrantId, Offer};
use porter_provider::Issuer;
use porter_service::Registry;

/// The issuers a bring-your-own client id may be set for (Google is a TODO, FINDINGS).
pub(crate) const ISSUERS: &[Issuer] = &[Issuer::Microsoft];

/// What a key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// `accounts.<id>.service.<kind>`.
    Service(AccountId, CapabilityKind),
    /// `accounts.<id>.state`.
    State(AccountId),
    /// `accounts.<id>.label`.
    Label(AccountId),
    /// `accounts.<id>.grant.<grant>`.
    Grant(AccountId, GrantId),
    /// `accounts.<id>.reauth`.
    Reauth(AccountId),
    /// `accounts.<id>.remove`.
    Remove(AccountId),
    /// `accounts.clients.<issuer>`.
    Client(Issuer),
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

/// The key `path` names, among the accounts of `registry` (an id may hold dots, so the account
/// is found by prefix, the longest id first).
pub(crate) fn parse(path: &str, registry: &Registry) -> Option<Key> {
    let rest = path.strip_prefix("accounts.")?;
    if let Some(issuer) = rest.strip_prefix("clients.") {
        return from_slug(issuer)
            .filter(|i| ISSUERS.contains(i))
            .map(Key::Client);
    }
    let mut ids: Vec<&AccountId> = registry.accounts.iter().map(|a| &a.id).collect();
    ids.sort_by_key(|id| std::cmp::Reverse(id.as_str().len()));
    ids.into_iter().find_map(|id| {
        let tail = rest.strip_prefix(id.as_str())?.strip_prefix('.')?;
        let id = id.clone();
        match tail {
            "state" => Some(Key::State(id)),
            "label" => Some(Key::Label(id)),
            "reauth" => Some(Key::Reauth(id)),
            "remove" => Some(Key::Remove(id)),
            _ => {
                if let Some(kind) = tail.strip_prefix("service.") {
                    return from_slug(kind).map(|k| Key::Service(id, k));
                }
                let grant = tail.strip_prefix("grant.")?;
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
        Key::Grant(id, grant) => format!("accounts.{id}.grant.{grant}"),
        Key::Reauth(id) => format!("accounts.{id}.reauth"),
        Key::Remove(id) => format!("accounts.{id}.remove"),
        Key::Client(issuer) => format!("accounts.clients.{}", slug(issuer)),
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
            Key::Client(_) => Exposure::Advanced,
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

fn account_keys(account: &Account, grants: &[Grant]) -> Vec<KeySpec> {
    let section = account.label.0.as_str();
    let id = &account.id;
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
            &Key::State(id.clone()),
            section,
            "Status".to_owned(),
            "",
            KeyKind::Fixed {
                variant: crate::account::state_slug(account.state).to_owned(),
            },
            toml::Value::String(crate::account::state_slug(account.state).to_owned()),
        ),
    ];
    let toggles = account
        .capabilities
        .iter()
        .filter_map(|claim| match &claim.offer {
            Offer::Present(_) => Some(claim.offer.kind()),
            Offer::Absent {
                kind,
                reason: porter_core::AbsentReason::TurnedOff,
            } => Some(*kind),
            Offer::Absent { .. } => None,
        });
    for kind in toggles {
        keys.push(spec(
            &Key::Service(id.clone(), kind),
            section,
            slug(&kind),
            "Whether apps may use this service of the account.",
            KeyKind::Toggle {
                variants: ["on".to_owned(), "off".to_owned()],
            },
            toml::Value::String("on".to_owned()),
        ));
    }
    for grant in grants.iter().filter(|g| g.key.account == *id) {
        let what = match grant.decision {
            Decision::Allow => "allowed",
            Decision::Deny => "refused",
        };
        keys.push(spec(
            &Key::Grant(id.clone(), grant.id.clone()),
            section,
            format!(
                "{} {what}: {}",
                grant.key.app.name.as_str(),
                slug(&grant.key.kind)
            ),
            "Revoking asks the app again the next time it needs the account.",
            action("Revoke", ActionWeight::Plain),
            off(),
        ));
    }
    keys.push(spec(
        &Key::Reauth(id.clone()),
        section,
        "Sign in again".to_owned(),
        "",
        action("Sign in again", ActionWeight::Plain),
        off(),
    ));
    keys.push(spec(
        &Key::Remove(id.clone()),
        section,
        "Remove account".to_owned(),
        "Deletes the account's stored credentials and every permission given for it.",
        action("Remove account", ActionWeight::Destructive),
        off(),
    ));
    keys
}

/// The schema of the module for `registry`.
pub(crate) fn schema(registry: &Registry) -> LiveSchema {
    let mut key: Vec<KeySpec> = registry
        .accounts
        .iter()
        .flat_map(|account| account_keys(account, &registry.grants))
        .collect();
    key.extend(ISSUERS.iter().map(|issuer| {
        spec(
            &Key::Client(*issuer),
            "Sign-in clients",
            format!("{} client id", slug(issuer)),
            "Bring your own OAuth client id; empty uses the one this build ships.",
            KeyKind::Text,
            toml::Value::String(String::new()),
        )
    }));
    LiveSchema { version: 1, key }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            accounts: vec![storage, mail_account()],
            toggles: vec![],
        }
    }

    #[test]
    fn every_key_is_under_accounts_and_hands_off_and_the_schema_checks() {
        let schema = schema(&registry());
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
            "accounts.clients.microsoft",
        ] {
            assert!(paths.contains(&want), "{want} in {paths:?}");
        }
    }

    #[test]
    fn a_destructive_action_is_marked_so() {
        let schema = schema(&registry());
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
        for key in &schema(&registry).key {
            let parsed = parse(&key.path.0, &registry).unwrap_or_else(|| panic!("{}", key.path.0));
            assert_eq!(path(&parsed), key.path.0);
        }
        for bad in [
            "accounts.nobody.remove",
            "accounts.fake-mail.service.teleport",
            "accounts.clients.google",
            "dock.size",
            "accounts.fake-mail.nonsense",
        ] {
            assert_eq!(parse(bad, &registry), None, "{bad}");
        }
    }
}
