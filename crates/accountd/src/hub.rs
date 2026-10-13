//! Who is told what changed: the manager's signals are unicast, sent only to the connections of
//! the two roles that read accounts, the shell (`SheetHost`) and Settings, never broadcast and
//! never to an app (no app holds anything of an account here: an app learns of an identity only
//! by asking `Identities1`). This file is the pure half: the events between two registries and
//! who hears each. `core` is the live half (the roster of connections, the signals sent).

use crate::account::state_slug;
use crate::settings_keys::{Key, path};
use ds_settings::schema::KeyPath;
use porter_core::AccountId;
use porter_dbus::CallerRole;
use porter_service::Registry;

/// One change a client may be told of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// `AccountAdded`.
    Added(AccountId),
    /// `AccountRemoved`.
    Removed(AccountId),
    /// `NeedsReauth`.
    NeedsReauth(AccountId),
    /// The account's `State` property changed: the reader is told to read it again.
    StateChanged(AccountId),
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
                if old.state != account.state {
                    out.push(Event::StateChanged(account.id.clone()));
                }
                let now_refused = account.state.needs_person();
                if now_refused && !old.state.needs_person() {
                    out.push(Event::NeedsReauth(account.id.clone()));
                }
            }
        }
    }
    out
}

/// Whether a connection of `role` is told of `event`: the shell and Settings, the only roles
/// that read an account, hear every one.
pub(crate) fn hears(role: CallerRole, _event: &Event) -> bool {
    matches!(role, CallerRole::SheetHost | CallerRole::Settings)
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
                    if let Some(words) = crate::settings_keys::expiry_text(account) {
                        news.push((row(Key::Expires(id.clone())), text(&words)));
                    }
                }
            }
            Event::NeedsReauth(_) => {}
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
    use porter_core::AccountState;
    use porter_fake::{mail_account, storage_account};

    fn registry(accounts: Vec<porter_core::Account>) -> Registry {
        Registry {
            accounts,
            grants: vec![],
            toggles: vec![],
        }
    }

    #[test]
    fn events_are_the_differences_and_only_the_shell_and_settings_hear_them() {
        let (storage, mail) = (storage_account(), mail_account());
        let before = registry(vec![storage.clone()]);

        let mut refused = storage.clone();
        refused.state = AccountState::NeedsReauth;
        let after = registry(vec![refused, mail.clone()]);
        let list = events(&before, &after);
        assert_eq!(
            list,
            vec![
                Event::StateChanged(storage.id.clone()),
                Event::NeedsReauth(storage.id.clone()),
                Event::Added(mail.id.clone())
            ]
        );
        for event in &list {
            for (role, heard) in [
                (CallerRole::SheetHost, true),
                (CallerRole::Settings, true),
                (CallerRole::App, false),
                (CallerRole::PorterDaemon, false),
                (CallerRole::Agent, false),
            ] {
                assert_eq!(hears(role, event), heard, "{role:?} {event:?}");
            }
        }

        let gone = registry(vec![mail]);
        assert!(events(&before, &gone).contains(&Event::Removed(storage.id)));
    }

    #[test]
    fn no_change_is_no_event() {
        let r = registry(vec![storage_account()]);
        assert!(events(&r, &r.clone()).is_empty());
    }

    #[test]
    fn the_settings_module_is_told_of_an_account_that_came_went_or_was_renamed() {
        let (storage, mail) = (storage_account(), mail_account());
        let before = registry(vec![storage.clone()]);
        let mut renamed = storage.clone();
        renamed.label = porter_core::AccountLabel("Work".into());
        let after = registry(vec![renamed, mail.clone()]);
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
