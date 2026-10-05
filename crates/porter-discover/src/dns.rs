//! The DNS seam (hickory behind it in the daemon, a fake in tests).

use porter_provider::DomainName;
use std::future::Future;

/// One SRV record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvRecord {
    /// Lower is tried first.
    pub priority: u16,
    /// Among equals, the share of tries.
    pub weight: u16,
    /// The port.
    pub port: u16,
    /// The host.
    pub target: DomainName,
}

/// One MX record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MxRecord {
    /// Lower is preferred.
    pub preference: u16,
    /// The mail host.
    pub host: DomainName,
}

/// Why a lookup gave no records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsFault {
    /// The name has none.
    NoRecords,
    /// The resolver could not be reached.
    Unreachable,
}

/// Resolves the record types discovery reads.
pub trait Dns: Send + Sync {
    /// SRV records for `name` (`_imaps._tcp.example.org`).
    fn srv(&self, name: &str) -> impl Future<Output = Result<Vec<SrvRecord>, DnsFault>> + Send;

    /// MX records for `domain`.
    fn mx(
        &self,
        domain: &DomainName,
    ) -> impl Future<Output = Result<Vec<MxRecord>, DnsFault>> + Send;
}
