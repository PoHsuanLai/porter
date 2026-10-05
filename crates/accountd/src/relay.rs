//! The authenticated relay behind `Tokens.OpenAuthenticated` (design/31 §7.1 item 9): a
//! socketpair, one end driven by `porter_proxy::relay` on a task, the other returned to the app.
//!
//! - The plan (and so the password or token) moves into the task and goes nowhere but to the
//!   endpoint; accountd keeps no copy, and the task ends, dropping it, when either side closes.
//! - The descriptor is returned only once the relay has authenticated: for IMAP, SMTP and
//!   ManageSieve the first byte the relay writes to the app is its synthesized greeting, which it
//!   sends after `AUTH` succeeded, so a server that refuses the credential brings a refusal and no
//!   descriptor. HTTP authenticates per request, so its descriptor is returned at once.
//! - The trusted roots are the platform's; [`RelayRoots::Only`] is the test seam that trusts a
//!   scratch CA instead, never the system store.

use porter_core::stream::ByteStream;
use porter_core::wire::Refusal;
use porter_core::{EndpointProtocol, RelayPlan};
use porter_proxy::{RelayEnd, RelayFault, RustlsConnect, TokioStream, relay};
use rustls_pki_types::CertificateDer;
use std::io;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::{OnceCell, oneshot};
use zbus::zvariant::OwnedFd;

/// How long the relay may take to dial and authenticate before the app is told `Unavailable`.
const AUTHENTICATE_WITHIN: Duration = Duration::from_secs(60);

/// Which certificates the relay trusts for the servers it dials.
#[derive(Debug, Clone, Default)]
pub enum RelayRoots {
    /// The platform's roots (`webpki-roots` when it has none).
    #[default]
    Platform,
    /// Only these (a test's scratch CA).
    Only(Vec<CertificateDer<'static>>),
}

/// The connector, built on first use: reading the platform's store touches the disk.
#[derive(Debug)]
pub(crate) struct Relays {
    roots: RelayRoots,
    connector: OnceCell<RustlsConnect>,
}

impl Relays {
    pub(crate) fn new(roots: RelayRoots) -> Self {
        Self {
            roots,
            connector: OnceCell::new(),
        }
    }

    async fn connector(&self) -> Result<RustlsConnect, Refusal> {
        self.connector
            .get_or_try_init(|| async {
                match self.roots.clone() {
                    RelayRoots::Only(roots) => Ok(RustlsConnect::trusting(roots)),
                    RelayRoots::Platform => tokio::task::spawn_blocking(RustlsConnect::platform)
                        .await
                        .map_err(|_| Refusal::Unavailable),
                }
            })
            .await
            .cloned()
    }

    /// Runs the relay for `plan` and returns the app's end once the relay has authenticated.
    pub(crate) async fn open(&self, plan: RelayPlan) -> Result<OwnedFd, Refusal> {
        let connect = self.connector().await?;
        let (client, server) = std::os::unix::net::UnixStream::pair().map_err(unavailable)?;
        server.set_nonblocking(true).map_err(unavailable)?;
        let server = UnixStream::from_std(server).map_err(unavailable)?;

        let (ready_tx, ready_rx) = oneshot::channel();
        let (end_tx, end_rx) = oneshot::channel();
        let mut app = Gated {
            inner: TokioStream(server),
            ready: Some(ready_tx),
        };
        if plan.endpoint.protocol() == Some(EndpointProtocol::Http) {
            app.signal();
        }
        tokio::spawn(async move {
            let end = relay(plan, app, &connect).await;
            let _ = end_tx.send(end);
        });

        let outcome = tokio::time::timeout(AUTHENTICATE_WITHIN, async {
            match ready_rx.await {
                Ok(()) => Ok(()),
                // The relay ended without a word to the app: why it did is its end.
                Err(_) => Err(end_rx.await.map_or(Refusal::Unavailable, refusal_of)),
            }
        })
        .await
        .unwrap_or(Err(Refusal::Unavailable));
        // On a refusal `client` drops here, so no descriptor leaves.
        outcome.map(|()| OwnedFd::from(std::os::fd::OwnedFd::from(client)))
    }
}

fn unavailable(_: io::Error) -> Refusal {
    Refusal::Unavailable
}

/// What the app is told when the relay ended before it could be used.
fn refusal_of(end: RelayEnd) -> Refusal {
    match end {
        RelayEnd::Failed(RelayFault::Refused) => Refusal::NeedsReauth,
        _ => Refusal::Unavailable,
    }
}

/// The app's end as the relay sees it; the first write to the app says the relay is up.
struct Gated {
    inner: TokioStream<UnixStream>,
    ready: Option<oneshot::Sender<()>>,
}

impl Gated {
    fn signal(&mut self) {
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(());
        }
    }
}

impl ByteStream for Gated {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).await
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_all(bytes).await?;
        self.signal();
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_refused_credential_asks_for_reauthentication() {
        let cases = [
            (RelayEnd::Failed(RelayFault::Refused), Refusal::NeedsReauth),
            (
                RelayEnd::Failed(RelayFault::Unreachable),
                Refusal::Unavailable,
            ),
            (RelayEnd::Failed(RelayFault::Tls), Refusal::Unavailable),
            (RelayEnd::Failed(RelayFault::Protocol), Refusal::Unavailable),
            (
                RelayEnd::Failed(RelayFault::ForeignOrigin),
                Refusal::Unavailable,
            ),
            (RelayEnd::Finished, Refusal::Unavailable),
        ];
        for (end, expected) in cases {
            assert_eq!(refusal_of(end), expected, "{end:?}");
        }
    }
}
