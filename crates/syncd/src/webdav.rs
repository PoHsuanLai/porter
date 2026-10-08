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
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use storage_webdav::{Clock, Dial, StreamHttp, StreamLimits, WebDavReplica};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// How long a relay may stay silent, once a request is waiting on it, before the exchange fails:
/// the wait for a response head, and the wait between two chunks of a body. A server that stops
/// answering (a half-open connection the far end never closes) would otherwise stall the
/// dataset forever.
pub const RELAY_IDLE: Duration = Duration::from_secs(60);

/// The most written under one idle time.
const WRITE_PIECE: usize = 64 * 1024;

/// The app's end of a relay, as a byte stream. Every read and every write gives up with
/// [`io::ErrorKind::TimedOut`] after the stream's idle time, which `storage-webdav` reports as
/// `HttpError::TimedOut` (and never retries on a new connection).
#[derive(Debug)]
pub struct RelayStream {
    end: End,
    idle: Duration,
}

#[derive(Debug)]
enum End {
    /// The descriptor of a Unix stream socket.
    Unix(UnixStream),
    /// An in-memory duplex (an app hosting accountd's core in process).
    Memory(DuplexEnd),
}

impl RelayStream {
    /// The stream an accepted relay is.
    pub fn from_relay(stream: AuthenticatedStream) -> io::Result<Self> {
        let end = match stream {
            AuthenticatedStream::Fd(fd) => {
                let std_stream = std::os::unix::net::UnixStream::from(fd);
                std_stream.set_nonblocking(true)?;
                End::Unix(UnixStream::from_std(std_stream)?)
            }
            AuthenticatedStream::Memory(end) => End::Memory(end),
        };
        Ok(Self {
            end,
            idle: RELAY_IDLE,
        })
    }

    /// The same stream giving up after `idle` of silence instead of [`RELAY_IDLE`].
    #[must_use]
    pub fn with_idle(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    /// A stream over a Unix socket the caller holds (a test's end of a socket pair).
    pub fn from_unix(stream: UnixStream) -> Self {
        Self {
            end: End::Unix(stream),
            idle: RELAY_IDLE,
        }
    }
}

/// `work`, or a timed-out error once `idle` has passed.
async fn within<T>(idle: Duration, work: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    tokio::time::timeout(idle, work)
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

impl ByteStream for RelayStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.end {
            End::Unix(stream) => within(self.idle, stream.read(buf)).await,
            End::Memory(end) => within(self.idle, end.read(buf)).await,
        }
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        // The idle time is per piece, so a large upload on a slow link is not cut short while
        // the far end keeps taking it.
        for piece in bytes.chunks(WRITE_PIECE) {
            match &mut self.end {
                End::Unix(stream) => within(self.idle, stream.write_all(piece)).await?,
                End::Memory(end) => within(self.idle, end.write_all(piece)).await?,
            }
        }
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        match &mut self.end {
            End::Unix(stream) => within(self.idle, stream.shutdown()).await,
            End::Memory(end) => within(self.idle, end.shutdown()).await,
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
