//! The person's other computers on their Tailscale network, as accountd lists them
//! (`org.quire.Tailnet1.Machines`). accountd answers an empty list when Tailscale is not an
//! account the person added, or is not working: so no computer is looked at, or connected to as a
//! candidate, while Tailscale is not an account in the accounts.

use porter_core::Machine;
use porter_dbus::{TailnetProxy, machine_from_dbus};
use std::future::Future;
use std::pin::Pin;

/// A boxed future, so the seam can be a trait object.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where the list of computers comes from.
pub trait Machines: std::fmt::Debug + Send + Sync + 'static {
    /// The other computers now; none when they cannot be asked.
    fn machines(&self) -> Boxed<'_, Vec<Machine>>;
}

/// accountd, over the session bus.
#[derive(Debug, Clone)]
pub struct BusMachines {
    connection: zbus::Connection,
}

impl BusMachines {
    /// accountd as `connection` reaches it.
    pub fn new(connection: zbus::Connection) -> Self {
        Self { connection }
    }
}

impl Machines for BusMachines {
    fn machines(&self) -> Boxed<'_, Vec<Machine>> {
        Box::pin(async move {
            let Ok(proxy) = TailnetProxy::new(&self.connection).await else {
                return Vec::new();
            };
            match proxy.machines().await {
                Ok(rows) => rows
                    .iter()
                    .filter_map(|row| machine_from_dbus(row).ok())
                    .collect(),
                Err(_) => Vec::new(),
            }
        })
    }
}

/// No computers: what a daemon without accountd's list has.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoMachines;

impl Machines for NoMachines {
    fn machines(&self) -> Boxed<'_, Vec<Machine>> {
        Box::pin(async { Vec::new() })
    }
}
