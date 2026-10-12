//! Telling listeners that the AI accounts changed.
//!
//! The hosted service saves its registry after every change (an account added or removed, a
//! permit given or revoked, a runtime reported, an agent's state). [`NewsStore`] wraps the file
//! store and wakes a watcher each time a save succeeds; the watcher reads the registry, compares
//! it with what it last told and says two things: inferd's `EnginesChanged` (a listener reads
//! `Places` again), and, on the settings module, `Changed(row, word)` for each row that moved, so
//! a pane that is open follows without a `Set`.

use super::host::KeyHost;
use super::rows::{Key, ServedSlot, path, since_text};
use ds_settings::schema::KeyPath;
use porter_core::store::Persisted;
use porter_service::{Registry, RegistryStore, StoreError};
use std::sync::Arc;
use tokio::sync::Notify;

/// A registry store that says when a save succeeded.
#[derive(Debug, Clone)]
pub struct NewsStore<R> {
    inner: R,
    saved: Arc<Notify>,
}

impl<R> NewsStore<R> {
    /// `inner`, waking `saved` after each successful save.
    pub fn new(inner: R, saved: Arc<Notify>) -> Self {
        Self { inner, saved }
    }
}

impl<R: RegistryStore> RegistryStore for NewsStore<R> {
    async fn load(&self) -> Result<Persisted, StoreError> {
        self.inner.load().await
    }

    async fn save(&self, state: &Persisted) -> Result<(), StoreError> {
        let saved = self.inner.save(state).await;
        if saved.is_ok() {
            self.saved.notify_one();
        }
        saved
    }
}

/// The settings rows that moved between `before` and `after`, with the word each is told as.
/// The module has no signal for "the key set changed"; a pane reads the schema again on any
/// `Changed`, so an account that appeared or went, and a new label, are said on a row of that
/// account: a new state or a new account is `accounts.<id>.state` with the state slug, an account
/// that went is the same row with `removed`, a new label is `.label`, and a permit for one
/// launcher session that came or went is its row with `added` or `removed`; a permit given
/// again with another scope or day says its `.scope` and `.since`.
pub(crate) fn row_news(before: &Registry, after: &Registry) -> Vec<(KeyPath, toml::Value)> {
    let row = |key: Key| KeyPath(path(&key));
    let text = |slug: &str| toml::Value::String(slug.to_owned());
    let mut news = Vec::new();
    for old in &before.accounts {
        if !after.accounts.iter().any(|a| a.id == old.id) {
            news.push((row(Key::State(old.id.clone())), text("removed")));
        }
    }
    for account in &after.accounts {
        let was = before.accounts.iter().find(|a| a.id == account.id);
        if was.is_none_or(|old| old.state != account.state) {
            news.push((
                row(Key::State(account.id.clone())),
                text(super::local::state_slug(account.state)),
            ));
        }
        if was.is_some_and(|old| old.label != account.label) {
            news.push((row(Key::Label(account.id.clone())), text(&account.label.0)));
        }
    }
    let grant_in = |registry: &Registry, id: &porter_core::GrantId| {
        registry.grants.iter().find(|g| g.id == *id).cloned()
    };
    let mut ids: Vec<porter_core::GrantId> = Vec::new();
    for g in after.grants.iter().chain(&before.grants) {
        let changed = grant_in(before, &g.id) != grant_in(after, &g.id);
        if changed && !ids.contains(&g.id) {
            ids.push(g.id.clone());
        }
    }
    for id in ids {
        let (was, now) = (grant_in(before, &id), grant_in(after, &id));
        if let Some(grant) = now
            .clone()
            .or_else(|| was.clone())
            .filter(|g| g.scope.session().is_some())
        {
            let key = Key::Grant(grant.key.account.clone(), grant.id.clone());
            news.push((row(key), text(if now.is_some() { "added" } else { "removed" })));
        }
        if let (Some(was), Some(now)) = (was, now) {
            let (account, grant) = (&now.key.account, &now.id);
            if was.scope.word() != now.scope.word() {
                news.push((
                    row(Key::Scope(account.clone(), grant.clone())),
                    text(now.scope.word()),
                ));
            }
            if since_text(&was) != since_text(&now) {
                news.push((
                    row(Key::Since(account.clone(), grant.clone())),
                    text(&since_text(&now)),
                ));
            }
        }
    }
    news
}

/// Watches `host`: each time `saved` fires, tells `engines` and the served settings module what
/// moved. Ends with the runtime.
pub fn watch<H: KeyHost>(
    host: Arc<H>,
    saved: Arc<Notify>,
    served: ServedSlot,
    engines: Arc<Notify>,
) {
    tokio::spawn(async move {
        let mut told = host.registry();
        loop {
            saved.notified().await;
            let now = host.registry();
            if now == told {
                continue;
            }
            let before = std::mem::replace(&mut told, now.clone());
            engines.notify_one();
            if let Some(module) = served.get() {
                for (row, value) in row_news(&before, &now) {
                    let _ = module.changed(&row, &value).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests;
