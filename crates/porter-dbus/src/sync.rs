//! `org.quire.Sync1` at `/org/quire/Sync1`: syncd's datasets and their state.

use crate::args::Details;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Sync1",
    default_service = "org.quire.Sync1",
    default_path = "/org/quire/Sync1"
)]
pub trait Sync {
    /// The datasets the caller may see (dataset kind slugs).
    fn datasets(&self) -> zbus::Result<Vec<String>>;
    /// One dataset's state by name (anchor age, pending, conflicts, paused).
    fn status(&self, dataset: &str) -> zbus::Result<Details>;
    /// Stops syncing one dataset.
    fn pause(&self, dataset: &str) -> zbus::Result<()>;
    /// Starts it again.
    fn resume(&self, dataset: &str) -> zbus::Result<()>;
    /// Transfer progress.
    #[zbus(signal)]
    fn progress(&self, dataset: &str, progress: Details) -> zbus::Result<()>;
    /// A conflict was stored for the owning app to resolve.
    #[zbus(signal)]
    fn conflict(&self, dataset: &str, conflict: Details) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct SyncSkeleton;

#[zbus::interface(name = "org.quire.Sync1")]
impl SyncSkeleton {
    fn datasets(&self) -> fdo::Result<Vec<String>> {
        Err(crate::introspect::frozen())
    }

    fn status(&self, dataset: String) -> fdo::Result<Details> {
        let _ = dataset;
        Err(crate::introspect::frozen())
    }

    fn pause(&self, dataset: String) -> fdo::Result<()> {
        let _ = dataset;
        Err(crate::introspect::frozen())
    }

    fn resume(&self, dataset: String) -> fdo::Result<()> {
        let _ = dataset;
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn progress(
        emitter: &SignalEmitter<'_>,
        dataset: &str,
        progress: Details,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn conflict(
        emitter: &SignalEmitter<'_>,
        dataset: &str,
        conflict: Details,
    ) -> zbus::Result<()>;
}
