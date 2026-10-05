//! The latchkey socket carrier: `porter_core::wire` frames over a Unix socket, for other
//! desktops and macOS. A named pipe on Windows is not built (there `SocketTransport` finds
//! nobody), nor is it without the `socket` feature (the carrier needs a runtime, which the pure
//! build does not reach).
//!
//! One connection per call, so a `Choose` waiting on a sheet blocks nothing else:
//!
//! - an accountd call is one `AccountsRequest` frame out and one `AccountsReply` frame back
//!   (a sheet's reply comes when the sheet ends; closing the connection abandons it);
//! - a session is a `LinkHello::Open` frame, then `ClientFrame`s out and `InferEvent`s back, as
//!   the bus's `Open` fd carries them (descriptors ride on the frame that names them); a refusal
//!   is the first event.
//!
//! The caller's identity is never in a frame: the agent derives it from the connection (peer
//! credentials). The agent is not built in this repo (accountd serves the bus; the socket front
//! belongs to the agent that hosts the core elsewhere), so this side is tested against a hand
//! written one.

#[cfg(all(unix, feature = "framed"))]
#[path = "socket/unix.rs"]
mod link;

#[cfg(not(all(unix, feature = "framed")))]
#[path = "socket/absent.rs"]
mod link;

use super::Transport;
use crate::authenticated::Relayed;
use crate::env::SocketPath;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, DataClass, EndpointUrl, GrantId, Need, Tier};
use porter_infer::{
    ClientFrame, InferEvent, InferSession, LinkHello, OpenFrame, OpenOptions, SessionError,
};

/// A connection to the agent on the latchkey socket (made for each call).
#[derive(Debug)]
pub struct SocketTransport {
    path: SocketPath,
}

impl SocketTransport {
    /// A transport to the agent at `path`.
    pub fn at(path: SocketPath) -> Self {
        Self { path }
    }

    /// Whether the agent accepts a connection now.
    pub(crate) async fn reachable(&self) -> bool {
        link::reachable(&self.path).await
    }
}

/// A streaming session on the agent's socket.
#[derive(Debug)]
pub struct SocketSession {
    link: link::Link,
}

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
    type Session = SocketSession;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        link::call(&self.path, request).await
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
        link::open_authenticated(&self.path, request).await
    }

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
        link::open(&self.path, hello)
            .await
            .map(|link| SocketSession { link })
    }
}
