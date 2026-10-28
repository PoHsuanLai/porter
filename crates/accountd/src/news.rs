//! A registry store that says when a save succeeded, so the clients are told what moved whoever
//! made the change (a sign-in that finished in the background, a permit given, a runtime
//! reported).

use porter_core::store::{Persisted, RegistryStore, StoreError};
use std::sync::Arc;
use tokio::sync::Notify;

/// A registry store that wakes a watcher each time a save succeeds.
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

#[cfg(test)]
mod tests {
    use super::*;
    use porter_fake::MemoryStore;

    #[tokio::test]
    async fn a_save_that_worked_wakes_the_watcher_and_one_that_failed_does_not() {
        let saved = Arc::new(Notify::new());
        let memory = MemoryStore::default();
        let store = NewsStore::new(memory.clone(), Arc::clone(&saved));
        memory.refusing(true);
        assert!(store.save(&Persisted::empty()).await.is_err());
        memory.refusing(false);
        store.save(&Persisted::empty()).await.expect("saved");
        // The one permit left by the good save is taken at once; a failed save left none behind it.
        tokio::time::timeout(test_kit::GENEROUS, saved.notified())
            .await
            .expect("woken by the save that worked");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), saved.notified())
                .await
                .is_err(),
            "the failed save woke nobody"
        );
    }
}
