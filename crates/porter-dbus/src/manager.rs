//! `org.quire.Accounts1.Manager` at `/org/quire/Accounts1`: adding an identity through the sheet
//! and the signals about the identities signed in. accountd keeps identities and nothing
//! grant-keyed, so there is no finding, choosing or granting here: an app that wants an identity
//! asks `Identities1.Offer`, and one that wants an AI key asks inferd's `Keys`.

use crate::args::Details;
use porter_dbus_macros::bus_interface;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

/// The interface, written once. `#[bus_interface]` makes [`ManagerProxy`] for its callers from this
/// trait, [`ManagerServer`] for the daemon to implement and [`ManagerServed`] to register.
#[bus_interface(
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
