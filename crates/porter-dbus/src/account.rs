//! `org.quire.Accounts1.Account`, one object per account at `account_path(id)`; readable only
//! with a grant for it.

use crate::args::Details;
use zbus::fdo;
use zbus::zvariant::OwnedObjectPath;

/// The caller's side.
#[zbus::proxy(
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

/// The daemon's side.
#[derive(Debug, Default)]
pub struct AccountSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Account")]
impl AccountSkeleton {
    #[zbus(property)]
    fn id(&self) -> fdo::Result<String> {
        Err(crate::introspect::frozen())
    }

    #[zbus(property)]
    fn provider(&self) -> fdo::Result<String> {
        Err(crate::introspect::frozen())
    }

    #[zbus(property)]
    fn label(&self) -> fdo::Result<String> {
        Err(crate::introspect::frozen())
    }

    #[zbus(property)]
    fn state(&self) -> fdo::Result<String> {
        Err(crate::introspect::frozen())
    }

    #[zbus(property)]
    fn capabilities(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    fn reauthenticate(
        &self,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (parent_window, options);
        Err(crate::introspect::frozen())
    }
}
