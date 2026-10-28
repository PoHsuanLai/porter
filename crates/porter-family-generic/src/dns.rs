//! DNS, shared: the lookups the generic family's mail search makes, handed in as a type-erased
//! value so the provider has no type parameter and reaches no resolver itself.

use porter_discover::{Dns, DnsFault, MxRecord, SrvRecord};
use porter_provider::DomainName;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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
