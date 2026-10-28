//! `Tailnet1` over the bus.

use crate::Bus;
use crate::failure::link_error;
use futures_util::StreamExt;
use porter_client::{Changes, LinkError, Malformed, TailnetLink};
use porter_dbus::accounts::{TailnetProxy, machine_from_dbus};
use porter_tailscale::Machine;

impl Bus {
    async fn tailnet_proxy(&self) -> Result<TailnetProxy<'static>, LinkError> {
        self.accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| link_error(&e))
    }
}

impl TailnetLink for Bus {
    async fn machines(&self) -> Result<Vec<Machine>, LinkError> {
        self.tailnet_proxy()
            .await?
            .machines()
            .await
            .map_err(|e| link_error(&e))?
            .iter()
            .map(|row| machine_from_dbus(row).map_err(|_| LinkError::Malformed(Malformed::Row)))
            .collect()
    }

    async fn watch(&self) -> Result<Changes<()>, LinkError> {
        let proxy = self.tailnet_proxy().await?;
        let stream = proxy.receive_changed().await.map_err(|e| link_error(&e))?;
        // Reading the list once is how accountd counts this connection among those it tells.
        proxy.machines().await.map_err(|e| link_error(&e))?;
        Ok(Changes::of(stream.map(|_| ())))
    }
}
