//! A registry store and an audit sink that keep everything in memory and can be read back after
//! they moved into a service.

use porter_core::audit::AuditEntry;
use porter_core::store::Persisted;
use porter_service::{AuditSink, RegistryStore, StoreError};
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while the lock is held already failed the test that caused it.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A registry store in memory. Clones share what is stored.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    kept: Arc<Mutex<Option<Persisted>>>,
    saves: Arc<Mutex<usize>>,
    refusing: Arc<Mutex<bool>>,
}

impl MemoryStore {
    /// A store already holding `state`.
    pub fn holding(state: Persisted) -> Self {
        Self {
            kept: Arc::new(Mutex::new(Some(state))),
            ..Self::default()
        }
    }

    /// What was last saved, if anything.
    pub fn stored(&self) -> Option<Persisted> {
        lock(&self.kept).clone()
    }

    /// How many saves succeeded.
    pub fn saves(&self) -> usize {
        *lock(&self.saves)
    }

    /// From now on every save fails as an unwritable medium does (and `refusing(false)` ends it).
    pub fn refusing(&self, refuse: bool) {
        *lock(&self.refusing) = refuse;
    }
}

impl RegistryStore for MemoryStore {
    async fn load(&self) -> Result<Persisted, StoreError> {
        Ok(lock(&self.kept).clone().unwrap_or_else(Persisted::empty))
    }

    async fn save(&self, state: &Persisted) -> Result<(), StoreError> {
        if *lock(&self.refusing) {
            return Err(StoreError::Unavailable);
        }
        *lock(&self.kept) = Some(state.clone());
        *lock(&self.saves) += 1;
        Ok(())
    }
}

/// An audit sink that collects entries. Clones share them.
#[derive(Debug, Clone, Default)]
pub struct RecordingAudit(Arc<Mutex<Vec<AuditEntry>>>);

impl RecordingAudit {
    /// Every entry so far, in order.
    pub fn entries(&self) -> Vec<AuditEntry> {
        lock(&self.0).clone()
    }
}

impl AuditSink for RecordingAudit {
    fn record(&self, entry: AuditEntry) {
        lock(&self.0).push(entry);
    }
}
