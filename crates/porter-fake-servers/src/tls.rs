//! The scratch CA and its leaf certificate (`fixtures/regen.sh` makes them with the openssl
//! CLI). TEST-ONLY key material: the leaf is valid for `localhost`, `*.fake.test`, `127.0.0.1`
//! and `::1`, and the CA signs nothing else.

use rustls::crypto::ring::default_provider;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use std::sync::Arc;
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// The scratch CA, PEM. A client that should trust the fakes loads this and nothing else.
pub const CA_PEM: &str = include_str!("../fixtures/ca.pem");
const LEAF_PEM: &str = include_str!("../fixtures/leaf.pem");
const LEAF_KEY: &str = include_str!("../fixtures/leaf.key");

/// The DER of the scratch CA, for a client that builds its own root store.
pub fn ca_der() -> CertificateDer<'static> {
    CertificateDer::from_pem_slice(CA_PEM.as_bytes()).expect("fixtures/ca.pem holds a certificate")
}

/// What a fake serves TLS with.
pub fn acceptor() -> TlsAcceptor {
    let chain = CertificateDer::pem_slice_iter(LEAF_PEM.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixtures/leaf.pem holds certificates");
    let key =
        PrivateKeyDer::from_pem_slice(LEAF_KEY.as_bytes()).expect("fixtures/leaf.key holds a key");
    let config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default versions")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("the leaf matches its key");
    TlsAcceptor::from(Arc::new(config))
}

/// A client config trusting only the scratch CA.
pub fn client_config() -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots
        .add(ca_der())
        .expect("the scratch CA is a usable root");
    Arc::new(
        ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// A connector trusting only the scratch CA.
pub fn connector() -> TlsConnector {
    TlsConnector::from(client_config())
}

/// The name the leaf certificate answers to in tests.
pub fn server_name() -> ServerName<'static> {
    ServerName::try_from("localhost").expect("localhost is a valid name")
}
