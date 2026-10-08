//! The production `Connect` (feature `tls`): TCP, and TLS by rustls on ring. Certificate and
//! name checks are always on; there is no switch that turns them off. The trusted roots are the
//! platform's (`rustls-native-certs`), and `webpki-roots` when the platform offers none.

use crate::connect::{Connect, ConnectFault};
use porter_core::stream::ByteStream;
use porter_core::{Origin, Tls};
use rustls_pki_types::{CertificateDer, ServerName};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::{TlsConnector, client::TlsStream};

/// How long a dial may take before the endpoint counts as unreachable.
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a TLS handshake may take once the connection is up (a server that accepts and then
/// says nothing).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// A connection to an endpoint: plain until a `STARTTLS` upgrade, or TLS from the start.
#[derive(Debug)]
pub enum NetStream {
    /// No TLS (yet).
    Plain(TcpStream),
    /// TLS.
    Tls(Box<TlsStream<TcpStream>>),
}

impl ByteStream for NetStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            NetStream::Plain(stream) => stream.read(buf).await,
            NetStream::Tls(stream) => stream.read(buf).await,
        }
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            NetStream::Plain(stream) => stream.write_all(bytes).await,
            NetStream::Tls(stream) => stream.write_all(bytes).await,
        }
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        match self {
            NetStream::Plain(stream) => stream.shutdown().await,
            NetStream::Tls(stream) => stream.shutdown().await,
        }
    }
}

/// Dials with TCP and upgrades with rustls.
#[derive(Clone)]
pub struct RustlsConnect {
    connector: TlsConnector,
    handshake_timeout: Duration,
}

impl std::fmt::Debug for RustlsConnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RustlsConnect")
    }
}

fn config(roots: RootCertStore) -> Arc<ClientConfig> {
    Arc::new(
        ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

impl RustlsConnect {
    /// Trusting the platform's roots, or `webpki-roots` when the platform has none. Reads the
    /// platform store from disk: call it once, off the async threads where that matters.
    pub fn platform() -> Self {
        let mut roots = RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        let (added, _rejected) = roots.add_parsable_certificates(native.certs);
        if added == 0 {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        Self::trusting_store(roots)
    }

    /// Trusting only these roots (a test's scratch CA).
    pub fn trusting(roots: impl IntoIterator<Item = CertificateDer<'static>>) -> Self {
        let mut store = RootCertStore::empty();
        store.add_parsable_certificates(roots);
        Self::trusting_store(store)
    }

    fn trusting_store(roots: RootCertStore) -> Self {
        Self {
            connector: TlsConnector::from(config(roots)),
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }

    /// The same connector giving up on a handshake after `after` instead of the default.
    #[must_use]
    pub fn with_handshake_timeout(mut self, after: Duration) -> Self {
        self.handshake_timeout = after;
        self
    }

    async fn handshake(&self, stream: TcpStream, host: &str) -> Result<NetStream, ConnectFault> {
        let name = ServerName::try_from(host.to_owned()).map_err(|_| ConnectFault::Tls)?;
        // A server that accepts the connection and never answers the hello is unreachable, not a
        // refused certificate.
        tokio::time::timeout(self.handshake_timeout, self.connector.connect(name, stream))
            .await
            .map_err(|_| ConnectFault::Unreachable)?
            .map(|tls| NetStream::Tls(Box::new(tls)))
            .map_err(|_| ConnectFault::Tls)
    }
}

/// A socket for `address` that keeps its connection probed once it is up, so a peer that
/// vanished without a close (a half-open connection) is noticed by the kernel and the read fails
/// instead of waiting forever. The probe timers are the system's: tuning them needs `socket2`,
/// which the pinned block does not carry (an ask).
fn socket_for(address: SocketAddr) -> io::Result<TcpSocket> {
    let socket = match address {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };
    socket.set_keepalive(true)?;
    Ok(socket)
}

/// Connects to `host:port` over a keepalive socket, trying each address the name has in turn.
async fn connect_probed(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last = io::Error::from(io::ErrorKind::NotFound);
    for address in tokio::net::lookup_host((host, port)).await? {
        match socket_for(address)?.connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}

impl Connect for RustlsConnect {
    type Stream = NetStream;

    async fn dial(&self, origin: &Origin, tls: Tls) -> Result<NetStream, ConnectFault> {
        let tcp = tokio::time::timeout(DIAL_TIMEOUT, connect_probed(&origin.host, origin.port))
            .await
            .map_err(|_| ConnectFault::Unreachable)?
            .map_err(|_| ConnectFault::Unreachable)?;
        match tls {
            Tls::Implicit => self.handshake(tcp, &origin.host).await,
            Tls::StartTls | Tls::Plain => Ok(NetStream::Plain(tcp)),
        }
    }

    async fn upgrade(&self, stream: NetStream, host: &str) -> Result<NetStream, ConnectFault> {
        match stream {
            NetStream::Plain(tcp) => self.handshake(tcp, host).await,
            // Already TLS: a second handshake on it has no meaning.
            NetStream::Tls(_) => Err(ConnectFault::Tls),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::UrlScheme;
    use tokio::net::TcpListener;

    #[test]
    fn a_dialed_socket_asks_the_kernel_to_probe_the_connection() {
        let v4 = socket_for("127.0.0.1:1".parse().expect("address")).expect("socket");
        assert!(v4.keepalive().expect("option"));
        let v6 = socket_for("[::1]:1".parse().expect("address")).expect("socket");
        assert!(v6.keepalive().expect("option"));
    }

    #[tokio::test]
    async fn a_server_that_accepts_and_never_answers_the_hello_is_given_up_on() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("address").port();
        // Accepts and then says nothing, holding the connection open.
        let silent = tokio::spawn(async move {
            let held = listener.accept().await.expect("accept");
            std::future::pending::<()>().await;
            drop(held);
        });
        let connect =
            RustlsConnect::trusting([]).with_handshake_timeout(Duration::from_millis(150));
        let origin = Origin {
            scheme: UrlScheme::Imaps,
            host: "127.0.0.1".to_owned(),
            port,
        };
        let started = std::time::Instant::now();
        let got = connect.dial(&origin, Tls::Implicit).await;
        assert!(matches!(got, Err(ConnectFault::Unreachable)), "{got:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        silent.abort();
    }
}
