//! What the password families reach the world through: the HTTP client, the clock that paces a
//! poll, and (for the generic family) DNS. Each is handed in as a shared, type-erased value so
//! the closed enum over families has no type parameter and no family reaches a runtime itself.

use porter_discover::{Dns, DnsFault, MxRecord, SrvRecord};
use porter_http::{SharedHttp, SharedSleep, Sleep};
use porter_provider::DomainName;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// How a sign-in that waits on the person (Login Flow v2) polls: each wait is one `first` longer
/// than the last up to `step`, and the sign-in gives up once `limit` has been waited out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    /// The first wait, and how much each later one grows by.
    pub first: Duration,
    /// The longest single wait.
    pub step: Duration,
    /// How long the person has in all.
    pub limit: Duration,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(2),
            step: Duration::from_secs(10),
            limit: Duration::from_secs(20 * 60),
        }
    }
}

impl Pacing {
    /// The wait before poll number `attempt` (counting from 0).
    pub fn wait(&self, attempt: u32) -> Duration {
        self.first
            .saturating_mul(attempt.saturating_add(1))
            .min(self.step)
    }
}

/// The seams a password family dials through.
#[derive(Debug, Clone)]
pub(crate) struct Io {
    pub(crate) http: SharedHttp,
    pub(crate) sleep: SharedSleep,
    pub(crate) pacing: Pacing,
}

impl Io {
    pub(crate) fn new(http: SharedHttp, sleep: impl Sleep + 'static) -> Self {
        Self {
            http,
            sleep: SharedSleep::new(sleep),
            pacing: Pacing::default(),
        }
    }
}

type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T, DnsFault>> + Send + 'a>>;

trait DynDns: Send + Sync {
    fn srv_boxed<'a>(&'a self, name: &'a str) -> Answer<'a, Vec<SrvRecord>>;
    fn mx_boxed<'a>(&'a self, domain: &'a DomainName) -> Answer<'a, Vec<MxRecord>>;
}

impl<D: Dns> DynDns for D {
    fn srv_boxed<'a>(&'a self, name: &'a str) -> Answer<'a, Vec<SrvRecord>> {
        Box::pin(self.srv(name))
    }

    fn mx_boxed<'a>(&'a self, domain: &'a DomainName) -> Answer<'a, Vec<MxRecord>> {
        Box::pin(self.mx(domain))
    }
}

/// Any [`Dns`], shared.
#[derive(Clone)]
pub struct SharedDns(Arc<dyn DynDns>);

impl SharedDns {
    /// Shares `dns`.
    pub fn new(dns: impl Dns + 'static) -> Self {
        Self(Arc::new(dns))
    }
}

impl std::fmt::Debug for SharedDns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedDns")
    }
}

impl Dns for SharedDns {
    fn srv(&self, name: &str) -> impl Future<Output = Result<Vec<SrvRecord>, DnsFault>> + Send {
        let name = name.to_owned();
        async move { self.0.srv_boxed(&name).await }
    }

    fn mx(
        &self,
        domain: &DomainName,
    ) -> impl Future<Output = Result<Vec<MxRecord>, DnsFault>> + Send {
        let domain = domain.clone();
        async move { self.0.mx_boxed(&domain).await }
    }
}
