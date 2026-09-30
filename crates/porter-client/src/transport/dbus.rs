//! The D-Bus carrier: porter-dbus proxies on the session bus; the caller's identity is what
//! the bus says about the connection. Frozen shape; not built yet.

use super::Transport;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest};
use porter_infer::{InferReply, InferRequest};

/// accountd and inferd on the session bus.
#[derive(Debug)]
pub struct DbusTransport {
    connection: porter_dbus::BusConnection,
}

impl DbusTransport {
    /// Over an open session-bus connection the app owns.
    pub fn over(connection: porter_dbus::BusConnection) -> Self {
        Self { connection }
    }
}

impl Transport for DbusTransport {
    async fn call(&self, _request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        let _ = &self.connection;
        todo!("map the request to its Manager/Grants/Tokens method; Request objects for sheets")
    }

    async fn infer(&self, _request: InferRequest) -> Result<InferReply, TransportError> {
        todo!("Inference1.Open, then frames on the returned fd")
    }
}
