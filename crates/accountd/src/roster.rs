//! Keeps the roster of connections honest: a connection that leaves the bus is forgotten, so a
//! unicast signal is never addressed to a name another process may be given later.

use crate::callers::Callers;
use crate::core::{Core, Host};
use std::pin::Pin;
use std::sync::Arc;
use zbus::Connection;
use zbus::export::futures_core::Stream;
use zbus::fdo::DBusProxy;

/// Starts the task that removes departed connections from `core`'s roster.
pub(crate) async fn watch<H: Host, C: Callers>(
    connection: &Connection,
    core: Arc<Core<H, C>>,
) -> zbus::Result<()> {
    let mut owners = DBusProxy::new(connection)
        .await?
        .receive_name_owner_changed()
        .await?;
    let core = Arc::downgrade(&core);
    tokio::spawn(async move {
        while let Some(signal) =
            std::future::poll_fn(|cx| Pin::new(&mut owners).poll_next(cx)).await
        {
            let Some(core) = core.upgrade() else { return };
            if let Ok(args) = signal.args()
                && args.new_owner().is_none()
            {
                core.left(args.name().as_str()).await;
            }
        }
    });
    Ok(())
}
