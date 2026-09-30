//! `org.quire.Accounts1.Manager` at `/org/quire/Accounts1`: find, choose and add accounts.
//! Signals go only to holders of a relevant grant (unicast).

use crate::args::{CandidateArg, Details, NeedArg};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Accounts1.Manager",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Accounts1"
)]
pub trait Manager {
    /// The granted accounts meeting `need` (no bulk enumeration).
    fn query(&self, need: &NeedArg, class: &str, usage: &str) -> zbus::Result<Vec<CandidateArg>>;
    /// `granted`, `available_needs_consent`, `denied`, `needs_account` or `unsupported`.
    fn availability(&self, need: &NeedArg, class: &str, usage: &str) -> zbus::Result<String>;
    /// Shows the chooser and consent sheet; returns a Request object.
    fn choose(
        &self,
        need: &NeedArg,
        class: &str,
        usage: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
    /// Shows the add-account sheet; returns a Request object.
    fn add_account(
        &self,
        provider_hint: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
    /// An account was added.
    #[zbus(signal)]
    fn account_added(&self, account: ObjectPath<'_>) -> zbus::Result<()>;
    /// An account was removed.
    #[zbus(signal)]
    fn account_removed(&self, account: ObjectPath<'_>) -> zbus::Result<()>;
    /// An account's effective capabilities changed.
    #[zbus(signal)]
    fn capability_changed(&self, account: ObjectPath<'_>) -> zbus::Result<()>;
    /// An account must be signed in again.
    #[zbus(signal)]
    fn needs_reauth(&self, account: ObjectPath<'_>) -> zbus::Result<()>;
    /// One of the receiver's grants changed or was revoked.
    #[zbus(signal)]
    fn grant_changed(&self, grant: &str) -> zbus::Result<()>;
}

/// The daemon's side: the interface's shape, every method answering `NotSupported`.
#[derive(Debug, Default)]
pub struct ManagerSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Manager")]
impl ManagerSkeleton {
    fn query(&self, need: NeedArg, class: String, usage: String) -> fdo::Result<Vec<CandidateArg>> {
        let _ = (need, class, usage);
        Err(crate::introspect::frozen())
    }

    fn availability(&self, need: NeedArg, class: String, usage: String) -> fdo::Result<String> {
        let _ = (need, class, usage);
        Err(crate::introspect::frozen())
    }

    fn choose(
        &self,
        need: NeedArg,
        class: String,
        usage: String,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (need, class, usage, parent_window, options);
        Err(crate::introspect::frozen())
    }

    fn add_account(
        &self,
        provider_hint: String,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (provider_hint, parent_window, options);
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn account_added(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn account_removed(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn capability_changed(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn needs_reauth(emitter: &SignalEmitter<'_>, account: ObjectPath<'_>)
    -> zbus::Result<()>;

    #[zbus(signal)]
    async fn grant_changed(emitter: &SignalEmitter<'_>, grant: &str) -> zbus::Result<()>;
}
