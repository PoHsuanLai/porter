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
    /// Settles one stored conflict of `dataset` (`<account>/<dataset>`): `conflict` is the number
    /// the `Conflict` signal carried (its `number` key, `CONFLICT_KEY_NUMBER`), `how` is
    /// `keep_local` (upload the local content over the replica's version) or `keep_remote` (take
    /// the replica's version, drop the local change); the next cycle does it. Only the dataset's
    /// owning app may call it. Errors: `InvalidArgs` for any other `how` (the message names the
    /// two words); `org.quire.Accounts1.Error.NoFittingAccount` when the caller sees no such
    /// dataset; `org.quire.Accounts1.Error.Denied` when it sees the dataset but does not own
    /// it; `org.quire.Sync1.Error.NoSuchConflict` when the conflict is unknown or already
    /// settled.
    fn resolve(&self, dataset: &str, conflict: i64, how: &str) -> zbus::Result<()>;
    /// Transfer progress.
    #[zbus(signal)]
    fn progress(&self, dataset: &str, progress: Details) -> zbus::Result<()>;
    /// A conflict was stored for the owning app to resolve. The details carry `number` (an `x`,
    /// what `resolve` takes), `item`, `local`, `remote` (`changed`, `deleted` or `exists`),
    /// `remote_version`, `remote_id`, `base` and `at`.
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

    /// Settles one stored conflict of `dataset` (`<account>/<dataset>`). `conflict` is the number
    /// the `Conflict` signal carried (its `number` key); `how` is `keep_local` (upload the local
    /// content over the replica's version) or `keep_remote` (take the replica's version and drop
    /// the local change), and the next cycle does it. Only the dataset's owning app may call it.
    /// Errors: `InvalidArgs` for any other `how`; `org.quire.Accounts1.Error.NoFittingAccount`
    /// when the caller sees no such dataset; `org.quire.Accounts1.Error.Denied` when it sees the
    /// dataset but does not own it; `org.quire.Sync1.Error.NoSuchConflict` when the conflict is
    /// unknown or already settled.
    fn resolve(&self, dataset: String, conflict: i64, how: String) -> fdo::Result<()> {
        let _ = (dataset, conflict, how);
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
