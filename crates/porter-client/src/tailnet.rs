//! The person's own computers on their Tailscale network (`org.quire.Tailnet1` on accountd), for
//! the shell, Settings and the terminal. Nothing here reads Tailscale: accountd does, once, for
//! the Tailscale account the person added.
//!
//! ```ignore
//! let tailnet = porter.tailnet();
//! let mut changes = tailnet.watch().await?;          // subscribe before reading the list
//! for machine in tailnet.machines().await? {          // Machine { node, name, dns, owner, .. }
//!     /* connect to machine.dns; key pins and consent by machine.node */
//! }
//! while changes.next().await.is_some() { /* read the list again */ }
//! ```

use crate::changes::Changes;
use crate::error::LinkError;
use crate::link::TailnetLink;
use porter_tailscale::Machine;
use std::sync::Arc;

/// accountd's list of the person's computers, as its callers see it.
#[derive(Debug)]
pub struct Tailnet<T> {
    pub(crate) link: Arc<T>,
}

impl<T> Clone for Tailnet<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: TailnetLink> Tailnet<T> {
    /// The other computers on the person's network, none for this one, in accountd's order. The
    /// list is empty with no Tailscale account, or one that is not working: the account's own
    /// state says why.
    pub async fn machines(&self) -> Result<Vec<Machine>, LinkError> {
        self.link.machines().await
    }

    /// The changes to the list, from now on: one item each time the list changed, with nothing in
    /// it; read the list again. It subscribes, then reads the list once so accountd counts this
    /// connection among those it tells.
    pub async fn watch(&self) -> Result<Changes<()>, LinkError> {
        self.link.watch().await
    }
}
