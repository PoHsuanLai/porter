//! The latchkey socket carrier: `porter_core::wire` frames over a Unix socket, for other
//! desktops and macOS, and over a named pipe on Windows (`socket/windows.rs`: no descriptors
//! there, so relays are refused and attached images must be inline; checked for
//! `x86_64-pc-windows-msvc` from Linux, never run). The agent's address, its single-instance lock
//! and how it is started are latchkey's (`SocketAgent` names the agent; the path is never ours to
//! write down). Without the `socket` feature there is no carrier (it needs a runtime and
//! latchkey, which the pure build does not reach): `SocketTransport` finds nobody.
//!
//! One connection per call, so a `Choose` waiting on a sheet blocks nothing else:
//!
//! - an accountd call is one `AccountsRequest` frame out and one `AccountsReply` frame back
//!   (a sheet's reply comes when the sheet ends; closing the connection abandons it);
//! - a session (feature `infer`) is a `LinkHello::Open` frame, then `ClientFrame`s out and
//!   `InferEvent`s back, as the bus's `Open` fd carries them (descriptors ride on the frame that
//!   names them); a refusal is the first event.
//!
//! The caller's identity is never in a frame: the agent derives it from the connection (peer
//! credentials). The agent is not built in this repo (accountd serves the bus; the socket front
//! belongs to the agent that hosts the core elsewhere), so this side is tested against a hand
//! written one and against a `latchkey::Agent::listen` door.

#[cfg(all(unix, feature = "socket"))]
#[path = "socket/unix.rs"]
mod link;

#[cfg(all(windows, feature = "socket"))]
#[path = "socket/windows.rs"]
mod link;

#[cfg(not(all(any(unix, windows), feature = "socket")))]
#[path = "socket/absent.rs"]
mod link;

use super::Transport;
use crate::authenticated::Relayed;
use crate::env::SocketAgent;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, EndpointUrl, GrantId};
#[cfg(feature = "infer")]
use porter_core::{DataClass, Need, Tier};
#[cfg(feature = "infer")]
use porter_infer::{
    ClientFrame, InferEvent, InferSession, LinkHello, OpenFrame, OpenOptions, SessionError,
};

/// A connection to the agent on the latchkey socket (made for each call).
#[derive(Debug)]
pub struct SocketTransport {
    agent: SocketAgent,
    door: link::Door,
}

impl SocketTransport {
    /// A transport to `agent`, at the address latchkey gives it.
    pub fn at(agent: SocketAgent) -> Self {
        let door = link::door(&agent);
        Self { agent, door }
    }

    /// Whether the agent accepts a connection now (starting it first if the app said to).
    pub(crate) async fn reachable(&self) -> bool {
        link::reachable(&self.door, &self.agent.start).await
    }
}

/// A streaming session on the agent's socket.
#[cfg(feature = "infer")]
#[derive(Debug)]
pub struct SocketSession {
    link: link::Link,
}

#[cfg(feature = "infer")]
impl InferSession for SocketSession {
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        self.link.send(frame).await
    }

    #[cfg(unix)]
    async fn send_attached(
        &mut self,
        frame: ClientFrame,
        attachments: Vec<std::os::fd::OwnedFd>,
    ) -> Result<(), SessionError> {
        self.link.send_attached(frame, attachments).await
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        self.link.next().await
    }
}

impl Transport for SocketTransport {
    #[cfg(feature = "infer")]
    type Session = SocketSession;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        link::call(&self.door, &self.agent.start, request).await
    }

    async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        let request = AccountsRequest::OpenAuthenticated {
            grant: grant.clone(),
            endpoint: endpoint.clone(),
        };
        link::open_authenticated(&self.door, &self.agent.start, request).await
    }

    async fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        let request = AccountsRequest::OpenLinked {
            grant: grant.clone(),
            origin: origin.clone(),
        };
        link::open_authenticated(&self.door, &self.agent.start, request).await
    }

    #[cfg(feature = "infer")]
    async fn open_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<SocketSession, TransportError> {
        let hello = LinkHello::Open(OpenFrame {
            need: need.clone(),
            class,
            tier,
            options: options.clone(),
        });
        link::open(&self.door, &self.agent.start, hello)
            .await
            .map(|link| SocketSession { link })
    }
}
