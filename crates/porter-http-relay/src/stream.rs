//! [`RelayStream`]: the app's end of a relay as a tokio stream.

use porter_client::AuthenticatedStream;
use porter_core::stream::DuplexEnd;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[cfg(unix)]
use tokio::net::UnixStream;

/// The app's end of a relay.
#[derive(Debug)]
pub struct RelayStream(End);

#[derive(Debug)]
enum End {
    /// The descriptor of a Unix stream socket.
    #[cfg(unix)]
    Unix(UnixStream),
    /// An in-memory duplex (an app hosting accountd's core in process).
    Memory(DuplexEnd),
}

impl RelayStream {
    /// The stream an accepted relay is.
    ///
    /// # Errors
    /// The descriptor could not be made a non-blocking socket.
    pub fn from_relay(stream: AuthenticatedStream) -> io::Result<Self> {
        let end = match stream {
            #[cfg(unix)]
            AuthenticatedStream::Fd(fd) => {
                let std_stream = std::os::unix::net::UnixStream::from(fd);
                std_stream.set_nonblocking(true)?;
                End::Unix(UnixStream::from_std(std_stream)?)
            }
            AuthenticatedStream::Memory(end) => End::Memory(end),
        };
        Ok(Self(end))
    }

    /// A stream over an in-memory end the caller holds (a test's end of
    /// [`porter_core::stream::duplex`]: no kernel I/O, so a paused clock is safe with it).
    pub fn from_memory(end: DuplexEnd) -> Self {
        Self(End::Memory(end))
    }

    /// A stream over a Unix socket the caller holds (a test's end of a socket pair).
    #[cfg(unix)]
    pub fn from_unix(stream: UnixStream) -> Self {
        Self(End::Unix(stream))
    }
}

impl AsyncRead for RelayStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.0 {
            #[cfg(unix)]
            End::Unix(stream) => Pin::new(stream).poll_read(cx, buf),
            End::Memory(end) => {
                let read = end.poll_read(cx, buf.initialize_unfilled());
                read.map_ok(|n| buf.advance(n))
            }
        }
    }
}

impl AsyncWrite for RelayStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.0 {
            #[cfg(unix)]
            End::Unix(stream) => Pin::new(stream).poll_write(cx, bytes),
            End::Memory(end) => end.poll_write(cx, bytes),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.0 {
            #[cfg(unix)]
            End::Unix(stream) => Pin::new(stream).poll_flush(cx),
            End::Memory(_) => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.0 {
            #[cfg(unix)]
            End::Unix(stream) => Pin::new(stream).poll_shutdown(cx),
            End::Memory(end) => {
                end.shutdown_now();
                Poll::Ready(Ok(()))
            }
        }
    }
}
