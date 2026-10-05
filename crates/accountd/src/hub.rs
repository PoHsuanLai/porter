//! Who is told what changed (design/31 §4.4): the manager's signals are unicast, sent only to
//! the connections of apps that hold a relevant grant, never broadcast. This file is the pure
//! half: the events between two registries and the apps each is for. `core` is the live half
//! (the roster of connections, the signals sent).

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

/// The apps `event` is for: the holders of a grant on its account (a removed account's holders
/// are found in `before`), or the holder of the grant a `GrantChanged` names.
pub(crate) fn audience(event: &Event, before: &Registry, after: &Registry) -> Vec<AppId> {
    match event {
        Event::Added(id)
        | Event::Removed(id)
        | Event::CapabilityChanged(id)
        | Event::NeedsReauth(id) => holders(id, before, after),
        Event::GrantChanged(grant) => before
            .grants
            .iter()
            .chain(&after.grants)
            .find(|g| g.id == *grant)
            .map(|g| vec![g.key.app.clone()])
            .unwrap_or_default(),
    }
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
                Event::NeedsReauth(storage.id.clone()),
                Event::Added(mail.id.clone())
            ]
        );
        assert_eq!(
            audience(&list[0], &before, &after),
            vec![app("org.example.Holder")]
        );
        assert!(audience(&list[1], &before, &after).is_empty());

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
}
