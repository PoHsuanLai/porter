//! RFC 6186 SRV records for mail: `_imaps`, `_imap`, `_pop3s`, `_submissions` (RFC 8314) and
//! `_submission`.
//!
//! Ported from mailo's `from_srv` and `best` in `crates/mail-proto/src/discover/mod.rs` and the
//! SRV step of `crates/mail-runtime/src/discover.rs` (MIT OR Apache-2.0, same author). The
//! implicit-TLS names are preferred; the STARTTLS names (`_imap`, `_submission`) serve only
//! when the implicit ones are absent, and are marked `Tls::StartTls`. The login is the whole
//! address, since SRV says nothing about it.

use crate::dns::{Dns, DnsFault, SrvRecord};
use crate::found::{DiscoverFault, Found, Pop3, Pop3Server, Source};
use crate::mail::{imap_claim, imap_endpoint, smtp_endpoint};
use porter_core::{LoginName, ServiceEndpoint, Tls};
use porter_provider::DomainName;

/// What the five mail SRV names answered; a name with no records is an empty list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SrvAnswers {
    /// `_imaps._tcp`: IMAP with implicit TLS.
    pub imaps: Vec<SrvRecord>,
    /// `_imap._tcp`: IMAP with STARTTLS.
    pub imap: Vec<SrvRecord>,
    /// `_pop3s._tcp`: POP3 with implicit TLS (RFC 6186; there is no STARTTLS name porter asks).
    pub pop3s: Vec<SrvRecord>,
    /// `_submissions._tcp`: submission with implicit TLS.
    pub submissions: Vec<SrvRecord>,
    /// `_submission._tcp`: submission with STARTTLS.
    pub submission: Vec<SrvRecord>,
}

/// The SRV names asked for `domain`, in the order of [`SrvAnswers`]' fields.
pub fn srv_names(domain: &DomainName) -> [String; 5] {
    ["_imaps", "_imap", "_pop3s", "_submissions", "_submission"]
        .map(|label| format!("{label}._tcp.{domain}"))
}

/// Asks the five names. A name with no records is empty; the lookup fails only when no name
/// could be asked at all.
pub async fn lookup_srv<D: Dns>(dns: &D, domain: &DomainName) -> Result<SrvAnswers, DnsFault> {
    let [imaps, imap, pop3s, submissions, submission] = srv_names(domain);
    let answers = [
        dns.srv(&imaps).await,
        dns.srv(&imap).await,
        dns.srv(&pop3s).await,
        dns.srv(&submissions).await,
        dns.srv(&submission).await,
    ];
    let reached = answers
        .iter()
        .any(|a| !matches!(a, Err(DnsFault::Unreachable)));
    let [imaps, imap, pop3s, submissions, submission] = answers.map(Result::unwrap_or_default);
    match reached {
        true => Ok(SrvAnswers {
            imaps,
            imap,
            pop3s,
            submissions,
            submission,
        }),
        false => Err(DnsFault::Unreachable),
    }
}

/// The servers SRV records give for `address`: one incoming and one outgoing, or
/// `NoServers`. POP3 is not looked at ([`Pop3::Ignore`]).
pub fn found_from_srv(address: &str, answers: &SrvAnswers) -> Result<Found, DiscoverFault> {
    found_from_srv_with(address, answers, Pop3::Ignore)
}

/// [`found_from_srv`] under `pop3`. With `Pop3::Report` the `_pop3s` servers are listed on
/// [`Found::pop3`] (best first), and a domain with POP3 and submission records but no IMAP is a
/// finding whose `endpoints` hold the submission server alone and whose `claims` are empty, as
/// for a document. Under `Pop3::Ignore` nothing changes: such a domain is `NoServers`.
pub fn found_from_srv_with(
    address: &str,
    answers: &SrvAnswers,
    pop3: Pop3,
) -> Result<Found, DiscoverFault> {
    let login = || LoginName(address.to_owned());
    let incoming = best(&answers.imaps, Tls::Implicit, login(), imap_endpoint)
        .or_else(|| best(&answers.imap, Tls::StartTls, login(), imap_endpoint));
    let outgoing = best(&answers.submissions, Tls::Implicit, login(), smtp_endpoint)
        .or_else(|| best(&answers.submission, Tls::StartTls, login(), smtp_endpoint));
    let pop3 = match pop3 {
        Pop3::Ignore => Vec::new(),
        Pop3::Report => pop3_servers(&answers.pop3s, address),
    };
    let (endpoints, claims) = match (incoming, outgoing) {
        (Some(imap), Some(smtp)) => (vec![imap, smtp], vec![imap_claim()]),
        (None, Some(smtp)) if !pop3.is_empty() => (vec![smtp], Vec::new()),
        _ => return Err(DiscoverFault::NoServers),
    };
    Ok(Found {
        endpoints,
        claims,
        source: Source::Srv,
        oauth: None,
        pop3,
    })
}

/// The `_pop3s` targets, implicit TLS, in the order `best` would pick them.
fn pop3_servers(records: &[SrvRecord], address: &str) -> Vec<Pop3Server> {
    ordered(records)
        .into_iter()
        .map(|r| Pop3Server {
            host: r.target.to_string(),
            port: r.port,
            tls: Tls::Implicit,
            login: LoginName(address.to_owned()),
        })
        .collect()
}

/// The preferred target of a service. Among several records the lowest priority wins, then the
/// highest weight, then the name: deterministic, where RFC 2782 picks among equals at random,
/// because the result is shown and asked about and must not change between asking and
/// answering. A target of `.` says the service is decidedly not offered (`DomainName` refuses
/// it), as does port 0.
fn best(
    records: &[SrvRecord],
    tls: Tls,
    login: LoginName,
    build: fn(String, u16, Tls, LoginName) -> Option<ServiceEndpoint>,
) -> Option<ServiceEndpoint> {
    ordered(records)
        .into_iter()
        .find_map(|r| build(r.target.to_string(), r.port, tls, login.clone()))
}

/// The records worth trying (port 0 dropped), most preferred first.
fn ordered(records: &[SrvRecord]) -> Vec<&SrvRecord> {
    let mut usable: Vec<&SrvRecord> = records.iter().filter(|r| r.port != 0).collect();
    usable.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(b.weight.cmp(&a.weight))
            .then(a.target.cmp(&b.target))
    });
    usable
}

#[cfg(test)]
mod tests;
