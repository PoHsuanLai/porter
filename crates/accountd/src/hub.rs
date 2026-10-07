//! Who is told what changed (design/31 §4.4): the manager's signals are unicast, sent only to
//! the connections of apps that hold a relevant grant, never broadcast. This file is the pure
//! half: the events between two registries and the apps each is for. `core` is the live half
//! (the roster of connections, the signals sent).

use crate::account::state_slug;
use crate::settings_keys::{Key, path};
use ds_settings::schema::KeyPath;
use porter_core::consent::Decision;
use porter_core::{AccountId, AccountState, AppId, GrantId};
use porter_service::Registry;

/// One change a client may be told of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// `AccountAdded`.
    Added(AccountId),
    /// `AccountRemoved`.
    Removed(AccountId),
    /// `CapabilityChanged`: the account's effective capabilities differ.
    CapabilityChanged(AccountId),
    /// `NeedsReauth`.
    NeedsReauth(AccountId),
    /// The account's `State` property changed: the shell is told to read it again.
    StateChanged(AccountId),
    /// `GrantChanged`: a grant was given, changed or revoked.
    GrantChanged(GrantId),
}

/// The events that take `before` to `after`, in a fixed order: removals, additions, changes.
pub(crate) fn events(before: &Registry, after: &Registry) -> Vec<Event> {
    let find = |registry: &Registry, id: &AccountId| {
        registry.accounts.iter().find(|a| a.id == *id).cloned()
    };
    let mut out = Vec::new();
    out.extend(
        before
            .accounts
            .iter()
            .filter(|a| find(after, &a.id).is_none())
            .map(|a| Event::Removed(a.id.clone())),
    );
    for account in &after.accounts {
        match find(before, &account.id) {
            None => out.push(Event::Added(account.id.clone())),
            Some(old) => {
                if old.capabilities != account.capabilities {
                    out.push(Event::CapabilityChanged(account.id.clone()));
                }
                if old.state != account.state {
                    out.push(Event::StateChanged(account.id.clone()));
                }
                let now_refused = account.state == AccountState::NeedsReauth;
                if now_refused && old.state != AccountState::NeedsReauth {
                    out.push(Event::NeedsReauth(account.id.clone()));
                }
            }
        }
    }
    let changed = |a: &Registry, b: &Registry| {
        a.grants
            .iter()
            .filter(|g| !b.grants.contains(g))
            .map(|g| g.id.clone())
            .collect::<Vec<_>>()
    };
    let mut grants = changed(after, before);
    for id in changed(before, after) {
        if !grants.contains(&id) {
            grants.push(id);
        }
    }
    out.extend(grants.into_iter().map(Event::GrantChanged));
    out
}

/// The apps that hold an allowing grant for `account` in either registry.
fn holders(account: &AccountId, before: &Registry, after: &Registry) -> Vec<AppId> {
    let mut apps: Vec<AppId> = Vec::new();
    for g in before.grants.iter().chain(&after.grants) {
        if g.key.account == *account && g.decision == Decision::Allow && !apps.contains(&g.key.app)
        {
            apps.push(g.key.app.clone());
        }
    }
    apps
}

/// Whether the shell (`CallerRole::SheetHost`) is told of `event` whatever grants it holds: the
/// shell notices an account that must be signed in again, and when it is well.
pub(crate) fn shell_hears(event: &Event) -> bool {
    matches!(event, Event::NeedsReauth(_) | Event::StateChanged(_))
}

/// The apps `event` is for: the holders of a grant on its account (a removed account's holders
/// are found in `before`), or the holder of the grant a `GrantChanged` names.
pub(crate) fn audience(event: &Event, before: &Registry, after: &Registry) -> Vec<AppId> {
    match event {
        Event::Added(id)
        | Event::Removed(id)
        | Event::CapabilityChanged(id)
        | Event::NeedsReauth(id)
        | Event::StateChanged(id) => holders(id, before, after),
        Event::GrantChanged(grant) => before
            .grants
            .iter()
            .chain(&after.grants)
            .find(|g| g.id == *grant)
            .map(|g| vec![g.key.app.clone()])
            .unwrap_or_default(),
    }
}

/// What the settings module's listeners are told as `Changed(key, value)`, in order. The module
/// has no signal for "the key set changed"; a pane reads the schema again on any `Changed`
/// (detent's `Followed::Changed`), so an account that appeared or went, and a new label, are
/// said on a row of that account:
///
/// - a new state, or an account that appeared: `accounts.<id>.state` with the state slug;
/// - an account that went: the same key with `removed` (the row is gone from the schema);
/// - a new label: `accounts.<id>.label` with the label.
pub(crate) fn settings_news(
    list: &[Event],
    before: &Registry,
    after: &Registry,
) -> Vec<(KeyPath, toml::Value)> {
    let row = |key: Key| KeyPath(path(&key));
    let text = |slug: &str| toml::Value::String(slug.to_owned());
    let mut news = Vec::new();
    for event in list {
        match event {
            Event::Removed(id) => news.push((row(Key::State(id.clone())), text("removed"))),
            Event::Added(id) | Event::StateChanged(id) => {
                if let Some(account) = after.accounts.iter().find(|a| a.id == *id) {
                    news.push((row(Key::State(id.clone())), text(state_slug(account.state))));
                }
            }
            _ => {}
        }
    }
    for account in &after.accounts {
        let renamed = before
            .accounts
            .iter()
            .find(|a| a.id == account.id)
            .is_some_and(|old| old.label != account.label);
        if renamed {
            news.push((row(Key::Label(account.id.clone())), text(&account.label.0)));
        }
    }
    news
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::consent::{Grant, GrantKey, GrantScope, Usage};
    use porter_core::{AppName, CapabilityKind, DataClass, Isolation, SpaceScope, UnixSeconds};
    use porter_fake::{mail_account, storage_account};

    fn app(name: &str) -> AppId {
        AppId {
            name: AppName::parse(name).expect("name"),
            isolation: Isolation::Flatpak,
        }
    }

    fn grant(id: &str, who: &str, account: &AccountId) -> Grant {
        Grant {
            id: GrantId::parse(id).expect("id"),
            key: GrantKey {
                app: app(who),
                account: account.clone(),
                kind: CapabilityKind::Storage,
                class: DataClass::Files,
                usage: Usage::Interactive,
                space: SpaceScope::Any,
            },
            scope: GrantScope::Always,
            decision: Decision::Allow,
            at: UnixSeconds(1),
        }
    }

    fn registry(accounts: Vec<porter_core::Account>, grants: Vec<Grant>) -> Registry {
        Registry {
            accounts,
            grants,
            toggles: vec![],
        }
    }

    #[test]
    fn events_are_the_differences_and_go_to_grant_holders_only() {
        let (storage, mail) = (storage_account(), mail_account());
        let held = grant("g1", "org.example.Holder", &storage.id);
        let before = registry(vec![storage.clone()], vec![held.clone()]);

        let mut refused = storage.clone();
        refused.state = AccountState::NeedsReauth;
        let after = registry(vec![refused, mail.clone()], vec![held.clone()]);
        let list = events(&before, &after);
        assert_eq!(
            list,
            vec![
                Event::StateChanged(storage.id.clone()),
                Event::NeedsReauth(storage.id.clone()),
                Event::Added(mail.id.clone())
            ]
        );
        assert_eq!(
            audience(&list[1], &before, &after),
            vec![app("org.example.Holder")]
        );
        assert!(audience(&list[2], &before, &after).is_empty());
        assert!(shell_hears(&list[0]) && shell_hears(&list[1]) && !shell_hears(&list[2]));

        let gone = registry(vec![mail], vec![]);
        let list = events(&before, &gone);
        assert!(list.contains(&Event::Removed(storage.id.clone())));
        assert!(list.contains(&Event::GrantChanged(held.id.clone())));
        assert_eq!(
            audience(&Event::Removed(storage.id), &before, &gone),
            vec![app("org.example.Holder")]
        );
        assert_eq!(
            audience(&Event::GrantChanged(held.id), &before, &gone),
            vec![app("org.example.Holder")]
        );
    }

    #[test]
    fn no_change_is_no_event() {
        let r = registry(vec![storage_account()], vec![]);
        assert!(events(&r, &r.clone()).is_empty());
    }

    #[test]
    fn the_settings_module_is_told_of_an_account_that_came_went_or_was_renamed() {
        let (storage, mail) = (storage_account(), mail_account());
        let before = registry(vec![storage.clone()], vec![]);
        let mut renamed = storage.clone();
        renamed.label = porter_core::AccountLabel("Work".into());
        let after = registry(vec![renamed, mail.clone()], vec![]);
        let key = |text: &str| KeyPath(text.to_owned());
        let said = |value: &str| toml::Value::String(value.to_owned());
        assert_eq!(
            settings_news(&events(&before, &after), &before, &after),
            vec![
                (key("accounts.fake-mail.state"), said("ok")),
                (key("accounts.fake-storage.label"), said("Work")),
            ]
        );
        assert_eq!(
            settings_news(&events(&after, &before), &after, &before),
            vec![
                (key("accounts.fake-mail.state"), said("removed")),
                (key("accounts.fake-storage.label"), said(&storage.label.0)),
            ]
        );
        assert!(settings_news(&events(&before, &before), &before, &before).is_empty());
    }
}
