//! The password tried at the incoming mail server before the account is added. A wrong password
//! or a server that cannot be reached ends the sign-in then, not as an account that is `Ok` until
//! the first mail fetch says otherwise.
//!
//! The login is the relay's own: [`porter_proxy::relay`] dials the endpoint, upgrades it and
//! authenticates (IMAP `LOGIN`/`AUTHENTICATE PLAIN`, POP3 `USER`/`PASS`), and only then writes to
//! the app. Here the "app" is a stream that notes that first write and refuses it, which ends the
//! relay at once: a session that logged in, and nothing else, is what the check needs.
//!
//! DAV does not come through here: `dav::discover` already sends a PROPFIND with the password.

use porter_core::stream::ByteStream;
use porter_core::{
    CapabilityKind, Credential, EndpointProtocol, RelayAuth, RelayPlan, ServiceEndpoint,
    sheet::SignInFault,
};
use porter_proxy::{Connect, RelayEnd, RelayFault, RustlsConnect, relay};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long the dial, the TLS handshake and the login may take in all. A person's sign-in on a
/// loaded computer or a slow network must not fail at 15 s for want of time; a server that never
/// answers is still reported after a minute.
const WITHIN: Duration = Duration::from_secs(60);

type Answer<'a> = Pin<Box<dyn Future<Output = Result<(), SignInFault>> + Send + 'a>>;

trait DynLogin: Send + Sync {
    fn check_boxed(&self, plan: RelayPlan) -> Answer<'_>;
}

/// A connector, shared, that logins are tried through.
#[derive(Clone)]
pub(super) struct SharedLogin(Arc<dyn DynLogin>);

impl std::fmt::Debug for SharedLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedLogin")
    }
}

impl SharedLogin {
    /// Logins tried through `connect`.
    pub(super) fn new(connect: impl Connect + 'static) -> Self {
        Self(Arc::new(Through(connect)))
    }

    /// Logins tried through the platform's roots, read on first use.
    pub(super) fn platform() -> Self {
        Self(Arc::new(Platform(OnceLock::new())))
    }

    /// Tries `credential` at the first incoming mail server of `endpoints`. Nothing to try (a
    /// token, a JMAP-only account) passes: the token's own call already proved it.
    pub(super) async fn check(
        &self,
        credential: &Credential,
        endpoints: &[ServiceEndpoint],
    ) -> Result<(), SignInFault> {
        let (Credential::Password(password), Some(endpoint)) = (credential, incoming(endpoints))
        else {
            return Ok(());
        };
        let plan = RelayPlan {
            endpoint: endpoint.clone(),
            kind: CapabilityKind::Mail,
            auth: RelayAuth::Password(password.clone()),
        };
        self.0.check_boxed(plan).await
    }
}

/// The endpoint the account reads mail from.
fn incoming(endpoints: &[ServiceEndpoint]) -> Option<&ServiceEndpoint> {
    endpoints.iter().find(|e| {
        matches!(
            e.protocol(),
            Some(EndpointProtocol::Imap | EndpointProtocol::Pop3)
        )
    })
}

/// What the person is told for a relay that ended without a login.
fn fault_of(fault: RelayFault) -> SignInFault {
    match fault {
        RelayFault::Refused => SignInFault::Refused,
        RelayFault::Unreachable | RelayFault::Tls => SignInFault::Unreachable,
        RelayFault::Protocol | RelayFault::ForeignOrigin => SignInFault::Unreadable,
    }
}

struct Through<C>(C);

impl<C: Connect> DynLogin for Through<C> {
    fn check_boxed(&self, plan: RelayPlan) -> Answer<'_> {
        Box::pin(login(plan, &self.0))
    }
}

/// The platform's roots, read once and off the async threads.
struct Platform(OnceLock<RustlsConnect>);

impl DynLogin for Platform {
    fn check_boxed(&self, plan: RelayPlan) -> Answer<'_> {
        Box::pin(async move {
            let connect = match self.0.get() {
                Some(connect) => connect.clone(),
                None => {
                    let read = tokio::task::spawn_blocking(RustlsConnect::platform)
                        .await
                        .map_err(|_| SignInFault::Unreachable)?;
                    self.0.get_or_init(|| read).clone()
                }
            };
            login(plan, &connect).await
        })
    }
}

async fn login<C: Connect>(plan: RelayPlan, connect: &C) -> Result<(), SignInFault> {
    let up = Arc::new(AtomicBool::new(false));
    let app = Probe(up.clone());
    let end = tokio::time::timeout(WITHIN, relay(plan, app, connect))
        .await
        .map_err(|_| SignInFault::Unreachable)?;
    match (up.load(Ordering::Acquire), end) {
        (true, _) => Ok(()),
        (false, RelayEnd::Failed(fault)) => Err(fault_of(fault)),
        // The relay ended in silence: the server hung up before it answered.
        (false, RelayEnd::Finished) => Err(SignInFault::Unreachable),
    }
}

/// The app's end of the relay: it never reads, and the first write (the relay's greeting, which it
/// sends only after logging in) is noted and refused, which ends the relay.
#[derive(Debug)]
struct Probe(Arc<AtomicBool>);

impl ByteStream for Probe {
    async fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        std::future::pending().await
    }

    async fn write_all(&mut self, _bytes: &[u8]) -> io::Result<()> {
        self.0.store(true, Ordering::Release);
        Err(io::ErrorKind::BrokenPipe.into())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        Ok(())
    }
}
