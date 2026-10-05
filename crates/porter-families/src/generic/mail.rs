//! Finding a generic mail server from an address: the search of `porter-discover`, and the
//! servers a person types when it finds none.

use crate::io::{Io, SharedDns};
use crate::password::declared;
use porter_core::sheet::SignInFault;
use porter_core::{Claim, EndpointUrl, Family, LoginName, ServiceEndpoint, Tls};
use porter_discover::{Outcome, discover_mail};
use porter_provider::{DomainName, ProviderSet, ProviderSpec};

/// What looking for a mail server came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Looked {
    /// Servers, and what the account can do there.
    Found(Vec<ServiceEndpoint>, Vec<Claim>),
    /// Nothing usable is published: ask the person for the server.
    Ask,
    /// Nothing could be reached to ask.
    Offline,
}

/// The domain of an address, when it is one.
pub(super) fn domain_of(address: &str) -> Option<DomainName> {
    let (local, domain) = address.rsplit_once('@')?;
    (!local.is_empty()).then(|| DomainName::parse(domain).ok())?
}

/// Searches for `address`'s servers.
pub(super) async fn look(
    io: &Io,
    dns: &SharedDns,
    providers: &ProviderSet,
    address: &str,
) -> Looked {
    match discover_mail(&io.http, dns, providers, address).await {
        Ok(Outcome::Servers(found)) => Looked::Found(found.endpoints, found.claims),
        // Another provider describes this address; this family cannot sign in to it, so the
        // person may still name the servers.
        Ok(Outcome::Provider(_)) => Looked::Ask,
        Err(not_found) if not_found.offline() => Looked::Offline,
        Err(_) => Looked::Ask,
    }
}

/// The servers at the host a person typed: IMAP over implicit TLS, SMTP submission with
/// STARTTLS, both logged in as the address.
pub(super) fn typed(
    host: &str,
    address: &str,
    spec: &ProviderSpec,
) -> Result<(Vec<ServiceEndpoint>, Vec<Claim>), SignInFault> {
    let host = DomainName::parse(host.trim()).map_err(|_| SignInFault::Unreadable)?;
    let login = LoginName(address.to_owned());
    let endpoint = |family, text: String, tls| {
        let endpoint = ServiceEndpoint {
            family,
            url: EndpointUrl::parse(&text).ok()?,
            tls,
            login: login.clone(),
        };
        endpoint.check().ok().map(|()| endpoint)
    };
    let endpoints = [
        endpoint(Family::Imap, format!("imaps://{host}:993"), Tls::Implicit),
        endpoint(Family::Smtp, format!("smtp://{host}:587"), Tls::StartTls),
    ]
    .into_iter()
    .collect::<Option<Vec<_>>>()
    .ok_or(SignInFault::Unreadable)?;
    Ok((endpoints, declared(spec)))
}

#[cfg(test)]
mod tests;
