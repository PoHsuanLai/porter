//! `org.quire.Accounts1.Grants` at `/org/quire/Accounts1`: the caller's own grants.

use crate::args::Details;
use zbus::fdo;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Accounts1.Grants",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Accounts1"
)]
pub trait Grants {
    /// The caller's grants: id and fields by name.
    fn list(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// Withdraws one of the caller's grants.
    fn revoke(&self, grant: &str) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct GrantsSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Grants")]
impl GrantsSkeleton {
    fn list(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    fn revoke(&self, grant: String) -> fdo::Result<()> {
        let _ = grant;
        Err(crate::introspect::frozen())
    }
}
