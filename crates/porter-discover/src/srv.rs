//! RFC 6186 SRV records for mail: `_imaps`, `_imap`, `_submissions` (RFC 8314) and
//! `_submission`.
//!
//! Ported from mailo's `from_srv` and `best` in `crates/mail-proto/src/discover/mod.rs` and the
//! SRV step of `crates/mail-runtime/src/discover.rs` (MIT OR Apache-2.0, same author). The
//! implicit-TLS names are preferred; the STARTTLS names (`_imap`, `_submission`) serve only
//! when the implicit ones are absent, and are marked `Tls::StartTls`. The login is the whole
//! address, since SRV says nothing about it.

use crate::dns::{Dns, DnsFault, SrvRecord};
use crate::found::{DiscoverFault, Found, Source};
use crate::mail::{imap_claim, imap_endpoint, smtp_endpoint};
use porter_core::{LoginName, ServiceEndpoint, Tls};
use porter_provider::DomainName;

/// What the four mail SRV names answered; a name with no records is an empty list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SrvAnswers {
    /// `_imaps._tcp`: IMAP with implicit TLS.
    pub imaps: Vec<SrvRecord>,
    /// `_imap._tcp`: IMAP with STARTTLS.
    pub imap: Vec<SrvRecord>,
    /// `_submissions._tcp`: submission with implicit TLS.
    pub submissions: Vec<SrvRecord>,
    /// `_submission._tcp`: submission with STARTTLS.
    pub submission: Vec<SrvRecord>,
}

/// The SRV names asked for `domain`, in the order of [`SrvAnswers`]' fields.
pub fn srv_names(domain: &DomainName) -> [String; 4] {
    ["_imaps", "_imap", "_submissions", "_submission"].map(|label| format!("{label}._tcp.{domain}"))
}

/// Asks the four names. A name with no records is empty; the lookup fails only when no name
/// could be asked at all.
pub async fn lookup_srv<D: Dns>(dns: &D, domain: &DomainName) -> Result<SrvAnswers, DnsFault> {
    let [imaps, imap, submissions, submission] = srv_names(domain);
    let answers = [
        dns.srv(&imaps).await,
        dns.srv(&imap).await,
        dns.srv(&submissions).await,
        dns.srv(&submission).await,
    ];
    let reached = answers
        .iter()
        .any(|a| !matches!(a, Err(DnsFault::Unreachable)));
    let [imaps, imap, submissions, submission] = answers.map(Result::unwrap_or_default);
    match reached {
        true => Ok(SrvAnswers {
            imaps,
            imap,
            submissions,
            submission,
        }),
        false => Err(DnsFault::Unreachable),
    }
}

/// The servers SRV records give for `address`: one incoming and one outgoing, or
/// `NoServers`.
pub fn found_from_srv(address: &str, answers: &SrvAnswers) -> Result<Found, DiscoverFault> {
    let login = || LoginName(address.to_owned());
    let incoming = best(&answers.imaps, Tls::Implicit, login(), imap_endpoint)
        .or_else(|| best(&answers.imap, Tls::StartTls, login(), imap_endpoint));
    let outgoing = best(&answers.submissions, Tls::Implicit, login(), smtp_endpoint)
        .or_else(|| best(&answers.submission, Tls::StartTls, login(), smtp_endpoint));
    match (incoming, outgoing) {
        (Some(imap), Some(smtp)) => Ok(Found {
            endpoints: vec![imap, smtp],
            claims: vec![imap_claim()],
            source: Source::Srv,
            oauth: None,
            pop3: Vec::new(),
        }),
        _ => Err(DiscoverFault::NoServers),
    }
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
    let mut usable: Vec<&SrvRecord> = records.iter().filter(|r| r.port != 0).collect();
    usable.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(b.weight.cmp(&a.weight))
            .then(a.target.cmp(&b.target))
    });
    usable
        .into_iter()
        .find_map(|r| build(r.target.to_string(), r.port, tls, login.clone()))
}

#[cfg(test)]
mod tests;
