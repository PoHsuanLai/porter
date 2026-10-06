//! The WebDAV replica as syncd runs it: connections come from accountd's
//! `Tokens.OpenAuthenticated`, so syncd never holds the account's credential (porter PLAN G3, D8).
//!
//! [`RelayDial`] asks accountd for one authenticated stream to the account's WebDAV endpoint each
//! time the replica needs a connection; the relay at the far end adds `Authorization`, speaks TLS
//! and reaches that endpoint's origin only. [`webdav_replica`] builds the replica of a dataset's
//! folder under that endpoint over them.

use porter_client::{Accounts, AuthenticatedStream, Transport};
use porter_core::stream::{ByteStream, DuplexEnd};
use porter_core::{EndpointUrl, GrantId, WebUrl};
use std::io;
use std::sync::Arc;
use storage_webdav::{Clock, Dial, StreamHttp, StreamLimits, WebDavReplica};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// The app's end of a relay, as a byte stream.
#[derive(Debug)]
pub enum RelayStream {
    /// The descriptor of a Unix stream socket.
    Unix(UnixStream),
    /// An in-memory duplex (an app hosting accountd's core in process).
    Memory(DuplexEnd),
}

impl RelayStream {
    /// The stream an accepted relay is.
    pub fn from_relay(stream: AuthenticatedStream) -> io::Result<Self> {
        match stream {
            AuthenticatedStream::Fd(fd) => {
                let std_stream = std::os::unix::net::UnixStream::from(fd);
                std_stream.set_nonblocking(true)?;
                Ok(Self::Unix(UnixStream::from_std(std_stream)?))
            }
            AuthenticatedStream::Memory(end) => Ok(Self::Memory(end)),
        }
    }
}

impl ByteStream for RelayStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.read(buf).await,
            Self::Memory(end) => end.read(buf).await,
        }
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.write_all(bytes).await,
            Self::Memory(end) => end.write_all(bytes).await,
        }
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.shutdown().await,
            Self::Memory(end) => end.shutdown().await,
        }
    }
}

/// Opens relays to one endpoint of one grant.
#[derive(Debug)]
pub struct RelayDial<T> {
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
}

impl<T> RelayDial<T> {
    /// Relays to `endpoint` under `grant`, opened through `accounts`.
    pub fn new(accounts: Arc<Accounts<T>>, grant: GrantId, endpoint: EndpointUrl) -> Self {
        Self {
            accounts,
            grant,
            endpoint,
        }
    }
}

impl<T: Transport> Dial for RelayDial<T> {
    type Stream = RelayStream;

    async fn dial(&self) -> Result<RelayStream, porter_http::HttpError> {
        let stream = self
            .accounts
            .open_authenticated(&self.grant, &self.endpoint)
            .await
            .map_err(|_| porter_http::HttpError::Unreachable)?;
        RelayStream::from_relay(stream).map_err(|_| porter_http::HttpError::Unreachable)
    }
}

/// Why a replica could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// The endpoint is not a web URL, or the folder makes it not one.
    #[error("the endpoint and folder do not make a web URL")]
    NotWeb,
}

/// The replica of `folder` (a path under the endpoint, `Photos/Originals`) of the account the
/// grant covers, over relays accountd opens to `endpoint`.
pub fn webdav_replica<T: Transport>(
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    folder: &str,
    clock: Clock,
) -> Result<WebDavReplica<StreamHttp<RelayDial<T>>>, BuildError> {
    let base = WebUrl::try_from(&endpoint).map_err(|_| BuildError::NotWeb)?;
    let url = WebUrl::parse(&format!(
        "{}/{}/",
        base.as_str().trim_end_matches('/'),
        folder.trim_matches('/')
    ))
    .map_err(|_| BuildError::NotWeb)?;
    let dial = RelayDial {
        accounts,
        grant,
        endpoint,
    };
    Ok(WebDavReplica::new(
        StreamHttp::new(dial, StreamLimits::default()),
        &url,
        clock,
    ))
}
