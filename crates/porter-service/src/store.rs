//! Where the registry lives between runs (porter PLAN §2.3): a seam, so the daemon writes a JSON
//! file atomically, an app hosting porter writes its platform state directory, and tests keep it
//! in memory.

use porter_core::store::{Persisted, StoreFault};
use std::fmt;
use std::future::Future;

/// Why the registry could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The stored document was refused (corrupt, from a newer build); the daemon must not start
    /// empty over it.
    Fault(StoreFault),
    /// The medium failed (a full disk, a read-only state directory).
    Unavailable,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Fault(fault) => fault.fmt(f),
            StoreError::Unavailable => f.write_str("registry store unavailable"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<StoreFault> for StoreError {
    fn from(fault: StoreFault) -> Self {
        StoreError::Fault(fault)
    }
}

/// Loads and saves the registry as one document.
pub trait RegistryStore: Send + Sync {
    /// The stored registry; an empty one where nothing was stored yet.
    fn load(&self) -> impl Future<Output = Result<Persisted, StoreError>> + Send;

    /// Replaces the stored registry, all or nothing.
    fn save(&self, state: &Persisted) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// Stores nothing: the registry lives for the life of the process.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStore;

impl RegistryStore for NoStore {
    async fn load(&self) -> Result<Persisted, StoreError> {
        Ok(Persisted::empty())
    }

    async fn save(&self, _state: &Persisted) -> Result<(), StoreError> {
        Ok(())
    }
}
