//! Finding a generic mail server from an address: the search of `porter-discover`. The servers a
//! person types when it finds none, and the ones a provider file names, are generic-proto's.

use crate::io::{Io, SharedDns};
use generic_proto::mail::domain_of;
use porter_capability::Claim;
use porter_core::ServiceEndpoint;
use porter_discover::{Found, Outcome, Source, discover_mail};
use porter_provider::{DomainName, ProviderSet};

/// What looking for a mail server came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Looked {
    /// Servers and what the account can do there.
    Found {
        /// The servers.
        endpoints: Vec<ServiceEndpoint>,
        /// What the account can do there.
        claims: Vec<Claim>,
        /// Whether the password is sent only after the person has seen the servers on the review
        /// and confirmed them: DNS SRV records naming a host outside the address's domain. Unsigned
        /// DNS can be answered by anyone on the path. Otherwise (the address's own domain or a
        /// database of documents: a provider file, the domain's autoconfig over HTTPS, the ISPDB,
        /// or SRV records naming hosts within the domain) the password is tried before the review.
        confirm_first: bool,
    },
    /// Nothing usable is published: ask the person for the server.
    Ask,
    /// Nothing could be reached to ask.
    Offline,
}

/// Whether the password is sent only after the person has confirmed `found`, the finding for an
/// address in `domain`: servers that only unsigned DNS names, outside the domain.
fn confirm_first(found: &Found, domain: &DomainName) -> bool {
    let within = |endpoint: &ServiceEndpoint| {
        DomainName::parse(&endpoint.url.origin().host).is_ok_and(|host| host.is_within(domain))
    };
    matches!(found.source, Source::Srv) && !found.endpoints.iter().all(within)
}

/// Searches for `address`'s servers.
pub(super) async fn look(
    io: &Io,
    dns: &SharedDns,
    providers: &ProviderSet,
    address: &str,
) -> Looked {
    match discover_mail(&io.http, dns, providers, address).await {
        Ok(Outcome::Servers(found)) => match domain_of(address) {
            Some(domain) => {
                let confirm_first = confirm_first(&found, &domain);
                Looked::Found {
                    endpoints: found.endpoints,
                    claims: found.claims,
                    confirm_first,
                }
            }
            None => Looked::Ask,
        },
        // Another provider describes this address; this family cannot sign in to it, so the
        // person may still name the servers.
        Ok(Outcome::Provider(_)) => Looked::Ask,
        Err(not_found) if not_found.offline() => Looked::Offline,
        Err(_) => Looked::Ask,
    }
}
