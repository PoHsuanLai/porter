//! Scratch listeners: a loopback port or a Unix socket in a scratch directory, and the one
//! connection type both produce.

use porter_fake::FakeAddress;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

/// Where a fake binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bind {
    /// An ephemeral port on `127.0.0.1`.
    Loopback,
    /// A socket file in this (scratch) directory, named after the fake.
    Socket(PathBuf),
}

/// A bound listener and the address it is reachable at.
#[derive(Debug)]
pub struct Listener {
    kind: Kind,
    address: FakeAddress,
}

#[derive(Debug)]
enum Kind {
    Tcp(TcpListener),
    Unix(UnixListener),
}

/// Who connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peer {
    /// A loopback peer, by its (ephemeral) address.
    Tcp(SocketAddr),
    /// A Unix peer.
    Unix,
}

/// One accepted or dialled connection.
#[derive(Debug)]
pub enum Conn {
    /// Over loopback.
    Tcp(TcpStream),
    /// Over a Unix socket.
    Unix(UnixStream),
}

impl Listener {
    /// Binds as `bind` says; `name` names the socket file for `Bind::Socket`.
    pub async fn bind(bind: &Bind, name: &str) -> io::Result<Self> {
        match bind {
            Bind::Loopback => {
                let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
                let port = listener.local_addr()?.port();
                Ok(Self {
                    kind: Kind::Tcp(listener),
                    address: FakeAddress::Loopback(port),
                })
            }
            Bind::Socket(dir) => {
                let path = dir.join(format!("{name}.sock"));
                let listener = UnixListener::bind(&path)?;
                Ok(Self {
                    kind: Kind::Unix(listener),
                    address: FakeAddress::Socket(path),
                })
            }
        }
    }

    /// Where it listens.
    pub fn address(&self) -> &FakeAddress {
        &self.address
    }

    /// The next connection.
    pub async fn accept(&self) -> io::Result<(Conn, Peer)> {
        match &self.kind {
            Kind::Tcp(l) => {
                let (stream, peer) = l.accept().await?;
                Ok((Conn::Tcp(stream), Peer::Tcp(peer)))
            }
            Kind::Unix(l) => {
                let (stream, _) = l.accept().await?;
                Ok((Conn::Unix(stream), Peer::Unix))
            }
        }
    }
}

/// Connects to a fake.
pub async fn dial(address: &FakeAddress) -> io::Result<Conn> {
    match address {
        FakeAddress::Loopback(port) => {
            Ok(Conn::Tcp(TcpStream::connect(("127.0.0.1", *port)).await?))
        }
        FakeAddress::Socket(path) => Ok(Conn::Unix(UnixStream::connect(path).await?)),
    }
}

/// The port of a loopback address; a Unix socket has none, so the caller named the wrong
/// kind of bind.
pub fn port_of(address: &FakeAddress) -> u16 {
    match address {
        FakeAddress::Loopback(port) => *port,
        FakeAddress::Socket(path) => {
            panic!(
                "{} is a Unix socket; this fake needs a loopback port",
                path.display()
            )
        }
    }
}

impl AsyncRead for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            Conn::Unix(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Conn::Tcp(s) => Pin::new(s).poll_write(cx, buf),
            Conn::Unix(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Tcp(s) => Pin::new(s).poll_flush(cx),
            Conn::Unix(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Conn::Unix(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
