//! The `Dns` seam over the system resolver (feature `io`).
//!
//! Ported from `SystemDns` in mailo's `crates/mail-runtime/src/discover.rs` (MIT OR
//! Apache-2.0, same author).

use crate::dns::{Dns, DnsFault, MxRecord, SrvRecord};
use hickory_resolver::TokioResolver;
use hickory_resolver::net::NetError;
use hickory_resolver::proto::rr::RData;
use porter_provider::DomainName;
use std::fmt;
use std::time::Duration;

/// How long one DNS query may take.
const PER_QUERY: Duration = Duration::from_secs(4);

/// The system's resolver, from `/etc/resolv.conf` or the platform's equivalent.
pub struct HickoryDns(TokioResolver);

impl fmt::Debug for HickoryDns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HickoryDns")
    }
}

impl HickoryDns {
    /// A resolver from the system's configuration, or why there is none.
    pub fn system() -> Result<Self, DnsFault> {
        let mut builder = TokioResolver::builder_tokio().map_err(|_| DnsFault::Unreachable)?;
        builder.options_mut().timeout = PER_QUERY;
        builder.options_mut().attempts = 1;
        builder.build().map(Self).map_err(|_| DnsFault::Unreachable)
    }
}

fn fault(e: NetError) -> DnsFault {
    match e.is_no_records_found() {
        true => DnsFault::NoRecords,
        false => DnsFault::Unreachable,
    }
}

impl Dns for HickoryDns {
    async fn srv(&self, name: &str) -> Result<Vec<SrvRecord>, DnsFault> {
        let lookup = self.0.srv_lookup(name).await.map_err(fault)?;
        Ok(lookup
            .answers()
            .iter()
            .filter_map(|record| match &record.data {
                RData::SRV(srv) => Some(SrvRecord {
                    priority: srv.priority,
                    weight: srv.weight,
                    port: srv.port,
                    // `.` (not offered) does not parse, so the record is dropped.
                    target: DomainName::parse(&srv.target.to_utf8()).ok()?,
                }),
                _ => None,
            })
            .collect())
    }

    async fn mx(&self, domain: &DomainName) -> Result<Vec<MxRecord>, DnsFault> {
        let lookup = self.0.mx_lookup(domain.as_str()).await.map_err(fault)?;
        Ok(lookup
            .answers()
            .iter()
            .filter_map(|record| match &record.data {
                RData::MX(mx) => Some(MxRecord {
                    preference: mx.preference,
                    host: DomainName::parse(&mx.exchange.to_utf8()).ok()?,
                }),
                _ => None,
            })
            .collect())
    }
}
