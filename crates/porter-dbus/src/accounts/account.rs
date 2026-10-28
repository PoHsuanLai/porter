//! `org.quire.Accounts1.Account`, one object per account at `account_path(id)`; readable only
//! with a grant for it.

use crate::Details;
use porter_dbus_macros::bus_interface;
use zbus::zvariant::OwnedObjectPath;

/// The interface, written once. `#[bus_interface]` makes [`AccountProxy`] for its callers from this
/// trait, [`AccountServer`] for the daemon to implement and [`AccountServed`] to register.
#[bus_interface(
    interface = "org.quire.Accounts1.Account",
    default_service = "org.quire.Accounts1"
)]
pub trait Account {
    /// The account id.
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;
    /// The provider id.
    #[zbus(property)]
    fn provider(&self) -> zbus::Result<String>;
    /// The label.
    #[zbus(property)]
    fn label(&self) -> zbus::Result<String>;
    /// `ok`, `needs_reauth`, `offline` or `limited`.
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    /// Why the account needs signing in again, as a word of a closed set
    /// (`porter_core::ReauthReason::word`), or the empty string when the state is not
    /// `needs_reauth` or porter knows no more than that the provider refused.
    #[zbus(property)]
    fn reauth_reason(&self) -> zbus::Result<String>;
    /// The effective capabilities the caller holds grants for: kind slug and fields.
    #[zbus(property)]
    fn capabilities(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// Signs the account in again; returns a Request object.
    fn reauthenticate(
        &self,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
}
