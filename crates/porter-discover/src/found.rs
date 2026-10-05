//! What a discovery step returns.

use porter_core::{Claim, ServiceEndpoint};

/// Where a finding came from, for the review step ("from the provider's autoconfig", "via MX").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// A provider file.
    Provider,
    /// The domain's autoconfig, or Thunderbird's database of them.
    Autoconfig,
    /// SRV records.
    Srv,
    /// An MX lead to a known provider.
    Mx,
    /// A `.well-known` URL.
    WellKnown,
    /// The JMAP session resource.
    JmapSession,
    /// Nextcloud's OCS capabilities.
    Ocs,
    /// A probe of local ports.
    Probe,
}

/// What discovery found for an address or a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The servers.
    pub endpoints: Vec<ServiceEndpoint>,
    /// What the account can do, at `Discovered` or `Probed` provenance.
    pub claims: Vec<Claim>,
    /// Where it came from.
    pub source: Source,
}

/// Why a step found nothing to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DiscoverFault {
    /// The domain answered and named no usable server.
    #[error("no servers named")]
    NoServers,
    /// Nothing could be reached to ask.
    #[error("unreachable")]
    Unreachable,
    /// The answer was not the format the step reads.
    #[error("unreadable answer")]
    Unreadable,
}
