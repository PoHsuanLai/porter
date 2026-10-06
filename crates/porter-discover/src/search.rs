//! The mail search: every source in order, stopping at the first usable answer.
//!
//! Ported from `search` and `fetch` in mailo's `crates/mail-runtime/src/discover.rs` (MIT OR
//! Apache-2.0, same author), over the [`Http`] and [`Dns`] seams instead of reqwest and
//! hickory. The order:
//!
//! 1. a provider porter knows that lists the address's domain (no network at all);
//! 2. the domain's own autoconfig document, its `.well-known` path, then the ISPDB;
//! 3. RFC 6186 SRV records;
//! 4. the MX records: a provider that claims an MX host, or the ISPDB document of the host's
//!    operator.
//!
//! HTTPS only, and no plain-HTTP fallback: `EndpointUrl`s here are all `https`, and a document
//! fetched in the clear could name any server at all. The `Http` implementation owns the
//! per-request timeout, size cap and redirect policy; this caps the document it will read.
//! Nothing is sent to a discovered server: the result goes to the review step.

use crate::autoconfig::{autoconfig_urls, domain_of, ispdb_url, parse_autoconfig};
use crate::dns::{Dns, DnsFault};
use crate::found::{DiscoverFault, Found, Source};
use crate::mx::{ProviderLead, ispdb_candidates, lookup_mx, provider_leads};
use crate::srv::{found_from_srv, lookup_srv};
use porter_core::WebUrl;
use porter_http::{Http, HttpError, HttpRequest, Method};
use porter_provider::{DomainMatch, ProviderSet};
use std::fmt;

/// The most a configuration document may weigh. Real ones are a few kilobytes.
const MAX_DOCUMENT: usize = 256 * 1024;

/// What the search came to: servers, or a provider porter already describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Servers a source named.
    Servers(Found),
    /// A provider file claims the address; its own discovery takes over.
    Provider(ProviderLead),
}

/// Why one step found nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// The source answered: there is nothing there (404, no records).
    Absent,
    /// No answer: DNS, the connection or TLS failed, or it timed out.
    Unreachable,
    /// An answer that is not a configuration: another status, or a document that did not parse.
    Malformed,
    /// A configuration that cannot be used, and why.
    Unusable(DiscoverFault),
}

/// One step of the search and what came of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tried {
    /// The URL fetched or the DNS name asked about.
    pub what: String,
    /// What came of it.
    pub miss: Miss,
}

/// The search found nothing usable. Every step is listed, so the user sees what was asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct NotFound {
    /// Every step, in order.
    pub tried: Vec<Tried>,
}

impl NotFound {
    /// Nothing answered at all: most likely this computer is offline.
    pub fn offline(&self) -> bool {
        !self.tried.is_empty() && self.tried.iter().all(|t| t.miss == Miss::Unreachable)
    }
}

impl fmt::Display for NotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offline() {
            true => f.write_str("could not reach anything to ask; is this computer offline?")?,
            false => f.write_str("no usable configuration was found")?,
        }
        self.tried.iter().try_for_each(|tried| {
            let why = match tried.miss {
                Miss::Absent => "nothing there",
                Miss::Unreachable => "no answer",
                Miss::Malformed => "not a configuration",
                Miss::Unusable(DiscoverFault::NoServers) => "names no usable server",
                Miss::Unusable(_) => "unusable",
            };
            write!(f, "\n  {}: {why}", tried.what)
        })
    }
}

/// Searches every source in order for `address`, stopping at the first usable answer.
pub async fn discover_mail<H: Http, D: Dns>(
    http: &H,
    dns: &D,
    providers: &ProviderSet,
    address: &str,
) -> Result<Outcome, NotFound> {
    let mut tried = Vec::new();
    match search(http, dns, providers, address, &mut tried).await {
        Some(outcome) => Ok(outcome),
        None => Err(NotFound { tried }),
    }
}

async fn search<H: Http, D: Dns>(
    http: &H,
    dns: &D,
    providers: &ProviderSet,
    address: &str,
    tried: &mut Vec<Tried>,
) -> Option<Outcome> {
    let Some(domain) = domain_of(address) else {
        tried.push(Tried {
            what: address.to_owned(),
            miss: Miss::Malformed,
        });
        return None;
    };

    // 1. A provider that lists the domain.
    if let Some(lead) = provider_leads(providers, &domain, &[]).into_iter().next() {
        return Some(Outcome::Provider(lead));
    }

    // 2. Documents.
    for url in autoconfig_urls(&domain, address) {
        if let Some(found) = document(http, &url, address, Source::Autoconfig, tried).await {
            return Some(Outcome::Servers(found));
        }
    }

    // 3. SRV.
    match lookup_srv(dns, &domain).await {
        Ok(answers) => match found_from_srv(address, &answers) {
            Ok(found) => return Some(Outcome::Servers(found)),
            Err(why) => tried.push(Tried {
                what: format!("SRV records for {domain}"),
                miss: match why {
                    DiscoverFault::NoServers => Miss::Absent,
                    other => Miss::Unusable(other),
                },
            }),
        },
        Err(fault) => tried.push(Tried {
            what: format!("SRV records for {domain}"),
            miss: dns_miss(fault),
        }),
    }

    // 4. MX.
    let records = match lookup_mx(dns, &domain).await {
        Ok(records) => records,
        Err(fault) => {
            tried.push(Tried {
                what: format!("MX {domain}"),
                miss: dns_miss(fault),
            });
            return None;
        }
    };
    if let Some(lead) = provider_leads(providers, &domain, &records)
        .into_iter()
        .find(|lead| lead.via == DomainMatch::Mx)
    {
        return Some(Outcome::Provider(lead));
    }
    let host = crate::mx::mx_hosts(&records).into_iter().next();
    let candidates = host
        .map(|host| ispdb_candidates(&host, &domain))
        .unwrap_or_default();
    for candidate in candidates {
        let Ok(url) = WebUrl::parse(&ispdb_url(&candidate)) else {
            continue;
        };
        if let Some(found) = document(http, &url, address, Source::Mx, tried).await {
            return Some(Outcome::Servers(found));
        }
    }
    tried.push(Tried {
        what: format!("MX {domain}"),
        miss: Miss::Absent,
    });
    None
}

fn dns_miss(fault: DnsFault) -> Miss {
    match fault {
        DnsFault::NoRecords => Miss::Absent,
        DnsFault::Unreachable => Miss::Unreachable,
    }
}

/// Fetches one document and reads it, recording why not when it cannot be used. `source`
/// replaces `Autoconfig` when the document was reached through an MX lead.
async fn document<H: Http>(
    http: &H,
    url: &WebUrl,
    address: &str,
    source: Source,
    tried: &mut Vec<Tried>,
) -> Option<Found> {
    let miss = match fetch(http, url).await {
        Ok(body) => match parse_autoconfig(&body, address) {
            Ok(found) => return Some(Found { source, ..found }),
            Err(DiscoverFault::Unreadable) => Miss::Malformed,
            Err(why) => Miss::Unusable(why),
        },
        Err(miss) => miss,
    };
    tried.push(Tried {
        what: url.to_string(),
        miss,
    });
    None
}

/// GETs a document, at most [`MAX_DOCUMENT`] of it.
async fn fetch<H: Http>(http: &H, url: &WebUrl) -> Result<String, Miss> {
    let response = http
        .send(HttpRequest::new(Method::Get, url.clone()))
        .await
        .map_err(|e| match e {
            HttpError::Unreachable | HttpError::Tls | HttpError::TimedOut => Miss::Unreachable,
            HttpError::TooLarge | HttpError::Malformed => Miss::Malformed,
        })?;
    match response.status.0 {
        404 | 410 => Err(Miss::Absent),
        _ if !response.status.is_success() => Err(Miss::Malformed),
        _ if response.body.len() > MAX_DOCUMENT => Err(Miss::Malformed),
        _ => String::from_utf8(response.body).map_err(|_| Miss::Malformed),
    }
}

#[cfg(test)]
mod tests;
