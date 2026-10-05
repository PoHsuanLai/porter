//! MX leads: where a domain's mail exchangers point.
//!
//! Ported from mailo's `from_mx` in `crates/mail-proto/src/discover/mod.rs` (MIT OR Apache-2.0,
//! same author). mailo's brand table is porter's provider files, so "an MX at a provider
//! porter knows" is `ProviderSet::claiming`; "an MX at another host" leads to that host's ISPDB
//! document. mailo found the host's registered domain with the public-suffix list, which is not
//! in the pinned block, so [`ispdb_candidates`] offers each parent of the host instead and the
//! ISPDB's 404 answers the question the list would have.

use crate::dns::{Dns, DnsFault, MxRecord};
use porter_core::ProviderId;
use porter_provider::{DomainMatch, DomainName, ProviderSet};

/// A provider porter knows, reached through an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLead {
    /// The provider.
    pub provider: ProviderId,
    /// Whether the address's domain or one of its MX hosts gave it away.
    pub via: DomainMatch,
    /// The MX host that gave it away; `None` for a domain match.
    pub exchanger: Option<DomainName>,
}

/// The MX hosts in preference order, ties by name. RFC 7505's null MX (`.`), which says the
/// domain takes no mail, is not a host and is left out.
pub fn mx_hosts(records: &[MxRecord]) -> Vec<DomainName> {
    let mut ordered: Vec<&MxRecord> = records.iter().collect();
    ordered.sort_by(|a, b| a.preference.cmp(&b.preference).then(a.host.cmp(&b.host)));
    ordered.into_iter().map(|r| r.host.clone()).collect()
}

/// The providers that claim `domain`, a listed domain before an MX hint, in file order.
pub fn provider_leads(
    set: &ProviderSet,
    domain: &DomainName,
    records: &[MxRecord],
) -> Vec<ProviderLead> {
    let hosts = mx_hosts(records);
    set.claiming(domain, &hosts)
        .into_iter()
        .map(|(spec, via)| ProviderLead {
            provider: spec.id.clone(),
            via,
            exchanger: match via {
                DomainMatch::Domain => None,
                DomainMatch::Mx => hosts
                    .iter()
                    .find(|h| spec.matching.mx_suffixes.iter().any(|s| h.is_within(s)))
                    .cloned(),
            },
        })
        .collect()
}

/// The domains whose ISPDB document may describe `host`'s operator: its parents, nearest first,
/// down to two labels, and never `domain` itself (its own document was already asked for).
pub fn ispdb_candidates(host: &DomainName, domain: &DomainName) -> Vec<DomainName> {
    let labels: Vec<&str> = host.as_str().split('.').collect();
    (1..labels.len().saturating_sub(1))
        .filter_map(|from| DomainName::parse(&labels[from..].join(".")).ok())
        .filter(|candidate| candidate != domain)
        .collect()
}

/// Asks DNS for `domain`'s MX records.
pub async fn lookup_mx<D: Dns>(dns: &D, domain: &DomainName) -> Result<Vec<MxRecord>, DnsFault> {
    dns.mx(domain).await
}

#[cfg(test)]
mod tests;
