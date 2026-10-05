//! The production `Connect` (feature `tls`): TCP, and TLS by rustls on ring. Certificate and
//! name checks are always on; there is no switch that turns them off. The trusted roots are the
//! platform's (`rustls-native-certs`), and `webpki-roots` when the platform offers none.

use crate::connect::{Connect, ConnectFault};
use porter_core::stream::ByteStream;
use porter_core::{Origin, Tls};
use rustls_pki_types::{CertificateDer, ServerName};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::{TlsConnector, client::TlsStream};

/// How long a dial may take before the endpoint counts as unreachable.
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

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
        }
    }

    async fn handshake(&self, stream: TcpStream, host: &str) -> Result<NetStream, ConnectFault> {
        let name = ServerName::try_from(host.to_owned()).map_err(|_| ConnectFault::Tls)?;
        self.connector
            .connect(name, stream)
            .await
            .map(|tls| NetStream::Tls(Box::new(tls)))
            .map_err(|_| ConnectFault::Tls)
    }
}

impl Connect for RustlsConnect {
    type Stream = NetStream;

    async fn dial(&self, origin: &Origin, tls: Tls) -> Result<NetStream, ConnectFault> {
        let address = (origin.host.as_str(), origin.port);
        let tcp = tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(address))
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
