//! The relay accountd runs for `Grants1.OpenAuthenticated` and `OpenLinked`. The app gets one end
//! of a Unix socket pair as a descriptor; this end is the relay's, dialled out to the endpoint
//! and signed in there with the credential the plan holds. The credential stays in this process:
//! no reply, signal or audit line carries it.

use porter_core::RelayPlan;
use porter_proxy::{RelayEnd, RelayFault, RustlsConnect, TokioStream, relay};
use porter_service_core::audit::ProxyOutcome;
use std::io;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::sync::OnceCell;

/// Runs relays on the daemon's tokio runtime over TCP and rustls. Certificate checks are always
/// on; the trusted roots are the platform's (`webpki-roots` when it has none), read from disk
/// once, on first use, off the async threads.
#[derive(Debug, Clone, Default)]
pub struct Relays {
    connector: Arc<OnceCell<RustlsConnect>>,
}

impl Relays {
    /// Relays that trust the platform's roots.
    pub fn platform() -> Self {
        Self::default()
    }

    /// Relays that dial with `connector` (a test's scratch CA, for one).
    pub fn with_connector(connector: RustlsConnect) -> Self {
        Self {
            connector: Arc::new(OnceCell::new_with(Some(connector))),
        }
    }

    /// Starts the relay for `plan` and returns the app's end of it. Must be called from inside a
    /// tokio runtime. The relay ends when either side finishes, and `ended` is told how, once.
    pub(crate) fn open(
        &self,
        plan: RelayPlan,
        ended: impl FnOnce(ProxyOutcome) + Send + 'static,
    ) -> io::Result<OwnedFd> {
        let (app, ours) = UnixStream::pair()?;
        let app = app.into_std()?;
        // The app decides how it reads its end.
        app.set_nonblocking(false)?;
        let cell = Arc::clone(&self.connector);
        tokio::spawn(async move {
            let connector = cell
                .get_or_try_init(|| async {
                    tokio::task::spawn_blocking(RustlsConnect::platform).await
                })
                .await;
            // A connector that could not be built ends the relay, and dropping `ours` finishes the
            // app's stream: the app sees nobody reachable.
            ended(match connector {
                Ok(connector) => outcome_of(relay(plan, TokioStream(ours), connector).await),
                Err(_) => ProxyOutcome::Tls,
            });
        });
        Ok(OwnedFd::from(app))
    }
}

/// How the audit record says a relay ended.
fn outcome_of(end: RelayEnd) -> ProxyOutcome {
    match end {
        RelayEnd::Finished => ProxyOutcome::Finished,
        RelayEnd::Failed(RelayFault::Refused) => ProxyOutcome::Refused,
        RelayEnd::Failed(RelayFault::Unreachable) => ProxyOutcome::Unreachable,
        RelayEnd::Failed(RelayFault::Tls) => ProxyOutcome::Tls,
        RelayEnd::Failed(RelayFault::ForeignOrigin) => ProxyOutcome::ForeignOrigin,
        // a fault a later porter-proxy adds is a protocol fault until it is told apart
        RelayEnd::Failed(_) => ProxyOutcome::Protocol,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_end_has_its_outcome() {
        const CASES: &[(RelayEnd, ProxyOutcome)] = &[
            (RelayEnd::Finished, ProxyOutcome::Finished),
            (RelayEnd::Failed(RelayFault::Refused), ProxyOutcome::Refused),
            (
                RelayEnd::Failed(RelayFault::Unreachable),
                ProxyOutcome::Unreachable,
            ),
            (RelayEnd::Failed(RelayFault::Tls), ProxyOutcome::Tls),
            (
                RelayEnd::Failed(RelayFault::Protocol),
                ProxyOutcome::Protocol,
            ),
            (
                RelayEnd::Failed(RelayFault::ForeignOrigin),
                ProxyOutcome::ForeignOrigin,
            ),
        ];
        for (end, outcome) in CASES {
            assert_eq!(outcome_of(*end), *outcome, "{end:?}");
        }
    }
}
