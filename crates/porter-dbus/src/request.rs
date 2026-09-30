//! `org.quire.Accounts1.Request`: one object per sheet shown, as portals do. `Response`
//! carries 0 (done), 1 (cancelled) or 2 (other), and the results by name.

use crate::args::Details;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

/// The caller's side.
#[zbus::proxy(
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

/// The daemon's side.
#[derive(Debug, Default)]
pub struct RequestSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Request")]
impl RequestSkeleton {
    fn close(&self) -> fdo::Result<()> {
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn response(
        emitter: &SignalEmitter<'_>,
        response: u32,
        results: Details,
    ) -> zbus::Result<()>;
}
