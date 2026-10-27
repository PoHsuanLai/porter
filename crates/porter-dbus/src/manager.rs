//! `org.quire.Accounts1.Manager` at `/org/quire/Accounts1`: adding an identity through the sheet
//! and the signals about the identities signed in. accountd keeps identities and nothing
//! grant-keyed, so there is no finding, choosing or granting here: an app that wants an identity
//! asks `Identities1.Offer`, and one that wants an AI key asks inferd's `Keys`.

use crate::args::Details;
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
    /// Shows the add-account sheet; returns a Request object.
    fn add_account(
        &self,
        provider_hint: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
    /// The accounts that need signing in again now, as object path and label: only the shell
    /// (`CallerRole::SheetHost`) may ask. Asking also joins the caller to the connections that
    /// are sent `NeedsReauth` for every account, and `State` changes of every account.
    fn needing_reauth(&self) -> zbus::Result<Vec<(OwnedObjectPath, String)>>;
    /// An account was added.
    #[zbus(signal)]
    fn account_added(&self, account: ObjectPath<'_>) -> zbus::Result<()>;
    /// An account was removed.
    #[zbus(signal)]
    fn account_removed(&self, account: ObjectPath<'_>) -> zbus::Result<()>;
    /// An account must be signed in again.
    #[zbus(signal)]
    fn needs_reauth(&self, account: ObjectPath<'_>) -> zbus::Result<()>;
}

/// The daemon's side: the interface's shape, every method answering `NotSupported`.
#[derive(Debug, Default)]
pub struct ManagerSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Manager")]
impl ManagerSkeleton {
    fn add_account(
        &self,
        provider_hint: String,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (provider_hint, parent_window, options);
        Err(crate::frozen::error())
    }

    fn needing_reauth(&self) -> fdo::Result<Vec<(OwnedObjectPath, String)>> {
        Err(crate::frozen::error())
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
    async fn needs_reauth(emitter: &SignalEmitter<'_>, account: ObjectPath<'_>)
    -> zbus::Result<()>;
}

/// Writes this interface's introspection into `xml`, as an `<interface>` element one level in:
/// the bus's introspection collects each interface's own.
pub(crate) fn introspect(xml: &mut String) {
    zbus::object_server::Interface::introspect_to_writer(&ManagerSkeleton, xml, 1);
}
