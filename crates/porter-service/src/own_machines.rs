//! The person's own computers on their tailnet, as the host knows them: asked when an add sheet
//! opens, so that the sheet's machine can tell whether plain `http` to a host is to one of
//! them (`porter_core::server_address`). A host with no view of the tailnet sets none, and only
//! this computer may then be plain.

use porter_core::Machine;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type Read = dyn Fn() -> Pin<Box<dyn Future<Output = Vec<Machine>> + Send>> + Send + Sync;

/// Where the list of the person's computers comes from.
#[derive(Clone)]
pub struct OwnMachines(Arc<Read>);

impl OwnMachines {
    /// A list read by `read` each time a sheet opens.
    pub fn new<F, Fut>(read: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Vec<Machine>> + Send + 'static,
    {
        Self(Arc::new(
            move || -> Pin<Box<dyn Future<Output = Vec<Machine>> + Send>> { Box::pin(read()) },
        ))
    }

    /// The computers now; none when they cannot be asked.
    pub(crate) async fn read(&self) -> Vec<Machine> {
        (self.0)().await
    }
}

impl std::fmt::Debug for OwnMachines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnMachines")
    }
}
