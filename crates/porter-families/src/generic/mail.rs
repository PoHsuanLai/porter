//! Finding a generic mail server from an address: the search of `porter-discover`, and the
//! servers a person types when it finds none.

use crate::io::{Io, SharedDns};
use crate::password::declared;
use porter_core::capability::{Capability, MailCap, MailTransport};
use porter_core::sheet::{Hop, MailServers, Security, SignInFault};
use porter_core::{Claim, EndpointUrl, Family, LoginName, Offer, ServiceEndpoint, Tls, UrlScheme};
use porter_discover::{Found, Outcome, Source, discover_mail};
use porter_provider::{DomainName, ProviderSet, ProviderSpec};

/// What looking for a mail server came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Looked {
    /// Servers, what the account can do there, and whether the password may be tried there
    /// before the person has seen them.
    Found(Vec<ServiceEndpoint>, Vec<Claim>, Vouched),
    /// Nothing usable is published: ask the person for the server.
    Ask,
    /// Nothing could be reached to ask.
    Offline,
}

/// Who vouches for the servers a search found, which decides when the password is first sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Vouched {
    /// The address's own domain or a database of documents (a provider file, the domain's
    /// autoconfig over HTTPS, the ISPDB), or SRV records naming hosts within the domain: the
    /// password is tried before the review.
    ByTheDomain,
    /// DNS SRV records naming a host outside the address's domain. Unsigned DNS can be answered
    /// by anyone on the path, so the password is sent only after the person has seen the
    /// servers on the review and confirmed them.
    Unconfirmed,
}

/// Who vouches for `found`, the finding for an address in `domain`.
fn vouched(found: &Found, domain: &DomainName) -> Vouched {
    let within = |endpoint: &ServiceEndpoint| {
        DomainName::parse(&endpoint.url.origin().host).is_ok_and(|host| host.is_within(domain))
    };
    match found.source {
        Source::Srv if !found.endpoints.iter().all(within) => Vouched::Unconfirmed,
        _ => Vouched::ByTheDomain,
    }
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
        Ok(Outcome::Servers(found)) => match domain_of(address) {
            Some(domain) => {
                let vouched = vouched(&found, &domain);
                Looked::Found(found.endpoints, found.claims, vouched)
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

/// One typed server as an endpoint: the scheme says the family and, with the security, whether
/// the connection is encrypted from the first byte (`imaps`, `pop3s`, `smtps`), upgraded (`imap`,
/// `pop3`, `smtp` with STARTTLS) or plain (this computer only; `check` refuses it anywhere else).
fn endpoint(family: Family, hop: &Hop, login: &LoginName) -> Option<ServiceEndpoint> {
    let (scheme, tls) = match (family, hop.security) {
        (Family::Imap, Security::Tls) => ("imaps", Tls::Implicit),
        (Family::Imap, Security::StartTls) => ("imap", Tls::StartTls),
        (Family::Imap, Security::Plain) => ("imap", Tls::Plain),
        (Family::Pop3, Security::Tls) => ("pop3s", Tls::Implicit),
        (Family::Pop3, Security::StartTls) => ("pop3", Tls::StartTls),
        (Family::Pop3, Security::Plain) => ("pop3", Tls::Plain),
        (Family::Smtp, Security::Tls) => ("smtps", Tls::Implicit),
        (Family::Smtp, Security::StartTls) => ("smtp", Tls::StartTls),
        (Family::Smtp, Security::Plain) => ("smtp", Tls::Plain),
        _ => return None,
    };
    let endpoint = ServiceEndpoint {
        family,
        url: EndpointUrl::parse(&format!("{scheme}://{}:{}", hop.host, hop.port)).ok()?,
        tls,
        login: login.clone(),
    };
    endpoint.check().ok().map(|()| endpoint)
}

/// The servers a person typed for an IMAP or POP3 account (`incoming`): that protocol and SMTP at
/// the hosts, ports and securities given, both logged in as the typed login name, or as the
/// address when there is none.
pub(super) fn typed(
    incoming: Family,
    servers: &MailServers,
    address: &str,
    spec: &ProviderSpec,
) -> Result<(Vec<ServiceEndpoint>, Vec<Claim>), SignInFault> {
    let login = LoginName(servers.login.clone().unwrap_or_else(|| address.to_owned()));
    let endpoints = [
        endpoint(incoming, &servers.incoming, &login),
        endpoint(Family::Smtp, &servers.outgoing, &login),
    ]
    .into_iter()
    .collect::<Option<Vec<_>>>()
    .ok_or(SignInFault::Unreadable)?;
    Ok((endpoints, claims_for(incoming, declared(spec))))
}

/// The file's claims for an account read by `incoming`: the file describes IMAP, so a POP3
/// account's mail claim names POP3, which an app that plans around the transport sees.
fn claims_for(incoming: Family, claims: Vec<Claim>) -> Vec<Claim> {
    if incoming != Family::Pop3 {
        return claims;
    }
    claims
        .into_iter()
        .map(|claim| match claim.offer {
            Offer::Present(Capability::Mail(mail)) => Claim {
                offer: Offer::Present(Capability::Mail(MailCap {
                    transport: MailTransport::Pop3,
                    ..mail
                })),
                ..claim
            },
            _ => claim,
        })
        .collect()
}

/// The TLS a file's endpoint URL means: `imaps`, `smtps` and `https` are implicit TLS; `imap`,
/// `smtp`, `pop3` and `sieve` upgrade with STARTTLS, except on this computer, where they may stay plain
/// (a fake server in a test). Anything else has no mail meaning.
fn tls_of_scheme(url: &EndpointUrl) -> Option<Tls> {
    let origin = url.origin();
    match origin.scheme {
        UrlScheme::Imaps
        | UrlScheme::Smtps
        | UrlScheme::Pop3s
        | UrlScheme::Https
        | UrlScheme::Sieves => Some(Tls::Implicit),
        UrlScheme::Imap
        | UrlScheme::Smtp
        | UrlScheme::Pop3
        | UrlScheme::Sieve
        | UrlScheme::Http
            if origin.is_loopback() =>
        {
            Some(Tls::Plain)
        }
        UrlScheme::Imap | UrlScheme::Smtp | UrlScheme::Pop3 | UrlScheme::Sieve => {
            Some(Tls::StartTls)
        }
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
