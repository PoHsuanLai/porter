//! The D-Bus carrier: porter-dbus proxies on the session bus; the caller's identity is what
//! the bus says about the connection. Frozen shape; not built yet.

use super::Transport;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, DataClass, Need, Tier};
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};

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

/// The fd `Inference1.Open` returned, framed with `porter_core::wire`'s envelope.
#[derive(Debug)]
pub struct DbusSession {
    _private: (),
}

impl InferSession for DbusSession {
    async fn send(&mut self, _frame: ClientFrame) -> Result<(), SessionError> {
        todo!("encode_frame the ClientFrame onto the fd; attach memfds with SCM_RIGHTS")
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        todo!("decode_frame the next InferEvent from the fd")
    }
}

impl Transport for DbusTransport {
    type Session = DbusSession;

    async fn call(&self, _request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        let _ = &self.connection;
        todo!("map the request to its Manager/Grants/Tokens method; Request objects for sheets")
    }

    async fn open(
        &self,
        _need: &Need,
        _class: DataClass,
        _tier: Tier,
    ) -> Result<DbusSession, TransportError> {
        todo!("Inference1.Open, then frames on the returned fd")
    }
}
