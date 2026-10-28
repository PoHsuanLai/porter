//! `org.quire.Accounts1.Request`: one object per sheet shown, as portals do. `Response`
//! carries 0 (done), 1 (cancelled) or 2 (other), and the results by name.

use crate::args::Details;
use porter_dbus_macros::bus_interface;

/// The interface, written once. `#[bus_interface]` makes [`RequestProxy`] for its callers from this
/// trait, [`RequestServer`] for the daemon to implement and [`RequestServed`] to register.
#[bus_interface(
    interface = "org.quire.Accounts1.Request",
    default_service = "org.quire.Accounts1"
)]
pub trait Request {
    /// Closes the sheet; no `Response` follows.
    fn close(&self) -> zbus::Result<()>;
    /// The answer.
    #[zbus(signal)]
    fn response(&self, response: u32, results: Details) -> zbus::Result<()>;
}
