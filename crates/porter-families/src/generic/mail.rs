//! Finding a generic mail server from an address: the search of `porter-discover`, and the
//! servers a person types when it finds none.

use crate::io::{Io, SharedDns};
use crate::password::declared;
use porter_core::sheet::SignInFault;
use porter_core::{Claim, EndpointUrl, Family, LoginName, ServiceEndpoint, Tls, UrlScheme};
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

/// The TLS a file's endpoint URL means: `imaps`, `smtps` and `https` are implicit TLS; `imap`,
/// `smtp` and `sieve` upgrade with STARTTLS, except on this computer, where they may stay plain
/// (a fake server in a test). Anything else has no mail meaning.
fn tls_of_scheme(url: &EndpointUrl) -> Option<Tls> {
    let origin = url.origin();
    match origin.scheme {
        UrlScheme::Imaps | UrlScheme::Smtps | UrlScheme::Https | UrlScheme::Sieves => {
            Some(Tls::Implicit)
        }
        UrlScheme::Imap | UrlScheme::Smtp | UrlScheme::Sieve | UrlScheme::Http
            if origin.is_loopback() =>
        {
            Some(Tls::Plain)
        }
        UrlScheme::Imap | UrlScheme::Smtp | UrlScheme::Sieve => Some(Tls::StartTls),
        UrlScheme::Http => None,
    }
}

/// The servers a provider file with `discovery = "fixed"` names: one endpoint per capability row
/// that has a URL, its family and TLS from the row and its scheme, logged in as the address. The
/// claims are the file's, once each (an SMTP row repeats its IMAP row's mail capability).
pub(super) fn fixed(
    address: &str,
    spec: &ProviderSpec,
) -> Result<(Vec<ServiceEndpoint>, Vec<Claim>), SignInFault> {
    let login = LoginName(address.to_owned());
    let endpoints = spec
        .capabilities
        .iter()
        .filter_map(|row| row.endpoint.as_ref().map(|url| (row.family, url)))
        .map(|(family, url)| {
            let url = EndpointUrl::parse(&url.0).map_err(|_| SignInFault::Unreadable)?;
            let endpoint = ServiceEndpoint {
                family,
                tls: tls_of_scheme(&url).ok_or(SignInFault::Unreadable)?,
                url,
                login: login.clone(),
            };
            endpoint
                .check()
                .map(|()| endpoint)
                .map_err(|_| SignInFault::Unreadable)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if endpoints.is_empty() {
        return Err(SignInFault::Unreadable);
    }
    let mut claims: Vec<Claim> = Vec::new();
    for claim in declared(spec) {
        if !claims.contains(&claim) {
            claims.push(claim);
        }
    }
    Ok((endpoints, claims))
}

#[cfg(test)]
mod tests;
