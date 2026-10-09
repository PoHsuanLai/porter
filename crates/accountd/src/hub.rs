//! Who is told what changed (design/31 §4.4): the manager's signals are unicast, sent only to
//! the connections of apps that hold a relevant grant, never broadcast. This file is the pure
//! half: the events between two registries and the apps each is for. `core` is the live half
//! (the roster of connections, the signals sent).

use crate::account::state_slug;
use crate::settings_keys::{Key, path, since_text};
use ds_settings::schema::KeyPath;
use porter_core::consent::{Decision, Grant};
use porter_core::{AccountId, AppId, GrantId};
use porter_service::{Registry, SyncClass, sync_allowed};

/// One change a client may be told of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// `AccountAdded`.
    Added(AccountId),
    /// `AccountRemoved`.
    Removed(AccountId),
    /// `CapabilityChanged`: the account's effective capabilities differ.
    CapabilityChanged(AccountId),
    /// `NeedsReauth`, also sent when an agent account goes `NeedsLogin`.
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
                // `NeedsLogin` (an agent that must be signed in, inside the agent) is told as
                // `NeedsReauth` is: the shell shows one notice for both.
                let now_refused = account.state.needs_person();
                if now_refused && !old.state.needs_person() {
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

/// Whether a porter daemon (`CallerRole::PorterDaemon`, one that has called accountd) is told of
/// `event` whatever grants it holds: an account appearing, going, changing what it offers or how
/// it stands. inferd lists the cloud AI accounts as places and tells its own listeners when that
/// list may have changed; a daemon already reads every account's id, label and state through
/// `Peer.Verdicts`, so these signals tell it nothing it cannot ask. Grants are not told: they are
/// per app.
pub(crate) fn daemon_hears(event: &Event) -> bool {
    matches!(
        event,
        Event::Added(_) | Event::Removed(_) | Event::CapabilityChanged(_) | Event::StateChanged(_)
    )
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

/// The grant `id` in `registry`, if it holds it.
fn grant_in<'a>(registry: &'a Registry, id: &GrantId) -> Option<&'a Grant> {
    registry.grants.iter().find(|g| g.id == *id)
}

/// What the settings module's listeners are told as `Changed(key, value)`, in order. The module
/// has no signal for "the key set changed"; a pane reads the schema again on any `Changed`
/// (detent's `Followed::Changed`), so an account that appeared or went, and a new label, are
/// said on a row of that account:
///
/// - a new state, or an account that appeared: `accounts.<id>.state` with the state slug;
/// - an account that went: the same key with `removed` (the row is gone from the schema);
/// - a new label: `accounts.<id>.label` with the label;
/// - a grant given again under its id with another scope or day: its `.scope` and `.since`
///   rows with the new value. A grant that came or went makes the pane read the schema again
///   only when it is a session grant, as its `Revoke` row always did, and the two rows come
///   and go with it; the account's `provider` and `group` rows never change for an account.
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
            // A grant for one launcher session comes and goes with the session: its row is said
            // to appear and to go (the other grants' rows are read when the pane is opened).
            Event::GrantChanged(id) => {
                let (was, now) = (grant_in(before, id), grant_in(after, id));
                if let Some(grant) = now.or(was).filter(|g| g.scope.session().is_some()) {
                    let row = row(Key::Grant(grant.key.account.clone(), grant.id.clone()));
                    news.push((row, text(if now.is_some() { "added" } else { "removed" })));
                }
                // A grant given again under its id keeps its rows but may say a new scope or a
                // new day: those two read-outs are told with their new value.
                if let (Some(was), Some(now)) = (was, now) {
                    let (account, grant) = (&now.key.account, &now.id);
                    if was.scope.word() != now.scope.word() {
                        news.push((
                            row(Key::Scope(account.clone(), grant.clone())),
                            text(now.scope.word()),
                        ));
                    }
                    if since_text(was) != since_text(now) {
                        news.push((
                            row(Key::Since(account.clone(), grant.clone())),
                            text(&since_text(now)),
                        ));
                    }
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
        // A sync row follows syncd's grant, however it came or went (the Revoke row of that
        // grant included).
        for class in SyncClass::ALL {
            let allowed = |registry: &Registry| sync_allowed(&registry.grants, &account.id, class);
            let now = allowed(after);
            if now != allowed(before) {
                news.push((
                    row(Key::Sync(account.id.clone(), class)),
                    text(if now { "on" } else { "off" }),
                ));
            }
        }
    }
    news
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::consent::{Grant, GrantKey, GrantScope, Usage};
    use porter_core::{
        AccountState, AppName, CapabilityKind, DataClass, Isolation, SpaceScope, UnixSeconds,
    };
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
        // A porter daemon hears an account come, go or change state, but not a reauth notice
        // (the shell's) or a grant (per app).
        assert!(daemon_hears(&list[0]) && !daemon_hears(&list[1]) && daemon_hears(&list[2]));
        assert!(!daemon_hears(&Event::GrantChanged(held.id.clone())));
        assert!(daemon_hears(&Event::Removed(storage.id.clone())));

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
    fn a_session_grants_row_is_said_to_come_and_go_and_another_grants_is_not() {
        let storage = storage_account();
        let always = grant("g-always", "org.example.Holder", &storage.id);
        let mut session = grant("g-session", "org.quire.Agent.claude-code", &storage.id);
        session.scope =
            GrantScope::Session(porter_core::LauncherSession::parse("sess-1").expect("session"));
        let (before, after) = (
            registry(vec![storage.clone()], vec![always.clone()]),
            registry(vec![storage.clone()], vec![always, session]),
        );
        let key = |text: &str| KeyPath(text.to_owned());
        let said = |value: &str| toml::Value::String(value.to_owned());
        let row = "accounts.fake-storage.grant.g-session";
        assert_eq!(
            settings_news(&events(&before, &after), &before, &after),
            vec![(key(row), said("added"))]
        );
        assert_eq!(
            settings_news(&events(&after, &before), &after, &before),
            vec![(key(row), said("removed"))]
        );
    }

    #[test]
    fn a_grant_given_again_under_its_id_tells_its_new_scope_and_day() {
        let storage = storage_account();
        let once = {
            let mut g = grant("g1", "org.example.Holder", &storage.id);
            g.scope = GrantScope::Once;
            g.at = porter_core::UnixSeconds(0);
            g
        };
        let always = {
            let mut g = once.clone();
            g.scope = GrantScope::Always;
            g.at = porter_core::UnixSeconds(86_400);
            g
        };
        let (before, after) = (
            registry(vec![storage.clone()], vec![once]),
            registry(vec![storage.clone()], vec![always]),
        );
        let key = |text: &str| KeyPath(text.to_owned());
        let said = |value: &str| toml::Value::String(value.to_owned());
        assert_eq!(
            settings_news(&events(&before, &after), &before, &after),
            vec![
                (key("accounts.fake-storage.grant.g1.scope"), said("always")),
                (
                    key("accounts.fake-storage.grant.g1.since"),
                    said("1970-01-02")
                ),
            ]
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
