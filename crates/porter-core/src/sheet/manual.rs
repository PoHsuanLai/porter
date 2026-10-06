//! The form for a mail server the person types by hand, asked when the lookup of an address
//! found none: how it speaks (IMAP, POP3 or JMAP), the server, its port and security, the
//! outgoing server, the login name when it is not the address, and for JMAP the session URL and,
//! instead of the password typed before, an API token.
//!
//! Everything about the form lives here and is pure, so every host gets the same rules: the form
//! as its answers shape it ([`manual_form`], [`refit`]), what is wrong with an answer
//! ([`form_problem`], one `FieldProblem` the host marks and uses to keep Continue off), and what
//! the answers mean ([`parse_manual`]). Ports follow protocol and security until the person
//! types one of their own.
//!
//! Security is TLS (from the first byte) or STARTTLS. `plain` is accepted for a host on this
//! computer only, the one place `ServiceEndpoint::check` lets a connection stay plain (a fake
//! server in a test); it is not a choice a host offers, and porter-discover's search never
//! offers it either.
//!
//! The server guesses (`imap.<domain>`, `smtp.<domain>`) are prefills the person edits, as
//! mailo's form showed them greyed; the answers decide.

use super::fields::{
    Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, first_missing,
};
use super::view::{FieldProblem, ProblemKind};
use crate::endpoint::{EndpointUrl, UrlScheme};
use crate::secret::SecretText;
use serde::{Deserialize, Serialize};

/// How a mail server speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// IMAP, with SMTP to send.
    Imap,
    /// POP3, with SMTP to send.
    Pop3,
    /// JMAP: one session URL, sending included.
    Jmap,
}

/// The protocols a host may offer, as the answer's text.
pub const PROTOCOL_CHOICES: &[&str] = &["imap", "pop3", "jmap"];

/// The securities a host may offer, as the answer's text.
pub const SECURITY_CHOICES: &[&str] = &["tls", "starttls"];

impl Protocol {
    fn slug(self) -> &'static str {
        match self {
            Protocol::Imap => "imap",
            Protocol::Pop3 => "pop3",
            Protocol::Jmap => "jmap",
        }
    }

    /// The protocol `text` names, when `choices` includes it.
    fn offered(text: &str, choices: &[&str]) -> Option<Protocol> {
        let all = [Protocol::Imap, Protocol::Pop3, Protocol::Jmap];
        all.into_iter()
            .find(|p| p.slug() == text && choices.contains(&text))
    }
}

/// How a connection to a typed server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    /// TLS from the first byte.
    Tls,
    /// Plain first, upgraded with STARTTLS before any credential is sent.
    StartTls,
    /// No TLS; this computer only.
    Plain,
}

impl Security {
    fn slug(self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::StartTls => "starttls",
            Security::Plain => "plain",
        }
    }

    fn parse(text: &str) -> Option<Security> {
        match text {
            "tls" => Some(Security::Tls),
            "starttls" => Some(Security::StartTls),
            "plain" => Some(Security::Plain),
            _ => None,
        }
    }
}

/// One server as typed: where, and how the connection is secured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    /// The host name or address, no scheme, port or path.
    pub host: String,
    /// The port, 1 to 65535.
    pub port: u16,
    /// How the connection is secured.
    pub security: Security,
}

/// An IMAP or POP3 account's two servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailServers {
    /// Where mail is read.
    pub incoming: Hop,
    /// Where mail is sent (SMTP).
    pub outgoing: Hop,
    /// The login name when it is not the address.
    pub login: Option<String>,
}

/// A JMAP account's server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JmapServer {
    /// The session URL.
    pub session: EndpointUrl,
    /// The login name when it is not the address.
    pub login: Option<String>,
    /// An API token, when the person gave one: it is presented as a bearer, and the password
    /// typed before is not used.
    pub token: Option<SecretText>,
}

/// What a filled server form means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Manual {
    /// IMAP and SMTP.
    Imap(MailServers),
    /// POP3 and SMTP.
    Pop3(MailServers),
    /// JMAP.
    Jmap(JmapServer),
}

impl Manual {
    /// The login name typed, when the person gave one.
    pub fn login(&self) -> Option<&str> {
        match self {
            Manual::Imap(m) | Manual::Pop3(m) => m.login.as_deref(),
            Manual::Jmap(j) => j.login.as_deref(),
        }
    }
}

/// The port a server of this kind usually listens on for `security`.
fn usual_port(protocol: Protocol, outgoing: bool, security: Security) -> u16 {
    match (outgoing, protocol, security) {
        (true, _, Security::Tls) => 465,
        (true, _, _) => 587,
        (false, Protocol::Pop3, Security::Tls) => 995,
        (false, Protocol::Pop3, _) => 110,
        (false, _, Security::Tls) => 993,
        (false, _, _) => 143,
    }
}

const USUAL_PORTS: [u16; 6] = [993, 995, 143, 110, 465, 587];

/// The labels a guessed host starts with, which follow the protocol.
const GUESS_LABELS: [&str; 3] = ["imap", "pop", "pop3"];

fn guess_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Pop3 => "pop",
        _ => "imap",
    }
}

fn spec(kind: FieldKind, presence: Presence, prefill: Option<String>) -> FieldSpec {
    FieldSpec {
        kind,
        entry: match kind {
            FieldKind::Token => Entry::Secret,
            _ => Entry::Plain,
        },
        presence,
        prefill,
    }
}

/// The form for `protocol`, its servers guessed from `domain` (the address's, when known) and
/// every port the usual one for TLS.
pub fn manual_form(protocol: Protocol, domain: Option<&str>) -> Vec<FieldSpec> {
    let guess = |label: &str| domain.map(|d| format!("{label}.{d}"));
    let mut fields = vec![spec(
        FieldKind::Protocol,
        Presence::Required,
        Some(protocol.slug().to_owned()),
    )];
    let port = |outgoing| Some(usual_port(protocol, outgoing, Security::Tls).to_string());
    match protocol {
        Protocol::Jmap => fields.extend([
            spec(
                FieldKind::SessionUrl,
                Presence::Required,
                domain.map(|d| format!("https://{d}/.well-known/jmap")),
            ),
            // Empty: the password typed before is the credential.
            spec(FieldKind::Token, Presence::Optional, None),
        ]),
        _ => fields.extend([
            spec(
                FieldKind::Server,
                Presence::Required,
                guess(guess_label(protocol)),
            ),
            spec(
                FieldKind::Security,
                Presence::Required,
                Some(Security::Tls.slug().to_owned()),
            ),
            spec(FieldKind::Port, Presence::Optional, port(false)),
            spec(FieldKind::OutgoingServer, Presence::Required, guess("smtp")),
            spec(
                FieldKind::OutgoingSecurity,
                Presence::Required,
                Some(Security::Tls.slug().to_owned()),
            ),
            spec(FieldKind::OutgoingPort, Presence::Optional, port(true)),
        ]),
    }
    fields.push(spec(FieldKind::Username, Presence::Optional, None));
    fields
}

fn text(answers: &[FieldAnswer], kind: FieldKind) -> Option<String> {
    answers
        .iter()
        .find(|a| a.kind == kind)
        .map(|a| match &a.value {
            FieldValue::Plain(t) => t.trim().to_owned(),
            FieldValue::Secret(s) => s.expose().trim().to_owned(),
        })
        .filter(|t| !t.is_empty())
}

/// A secret answer, when there is one.
fn secret(answers: &[FieldAnswer], kind: FieldKind) -> Option<SecretText> {
    answers
        .iter()
        .find(|a| a.kind == kind)
        .map(|a| match &a.value {
            FieldValue::Plain(t) => t.trim().to_owned(),
            FieldValue::Secret(s) => s.expose().trim().to_owned(),
        })
        .filter(|t| !t.is_empty())
        .map(SecretText::new)
}

/// Whether `fields` is the server form.
pub fn is_manual(fields: &[FieldSpec]) -> bool {
    fields.iter().any(|f| f.kind == FieldKind::Protocol)
}

/// A guessed host moved to the label of `protocol` (`imap.example.org` to `pop.example.org`);
/// any other host as it is.
fn retarget(host: &str, protocol: Protocol) -> String {
    match host.split_once('.') {
        Some((label, rest)) if GUESS_LABELS.contains(&label) && !rest.is_empty() => {
            format!("{}.{rest}", guess_label(protocol))
        }
        _ => host.to_owned(),
    }
}

/// The address's domain, read back from the guesses a form was made with, so a form reshaped for
/// another protocol guesses from the same domain.
fn domain_hint(fields: &[FieldSpec]) -> Option<String> {
    let prefill = |kind| {
        fields
            .iter()
            .find(|f| f.kind == kind)
            .and_then(|f| f.prefill.as_deref())
    };
    let after_label = |host: &str, labels: &[&str]| {
        let (label, rest) = host.split_once('.')?;
        (labels.contains(&label) && !rest.is_empty()).then(|| rest.to_owned())
    };
    prefill(FieldKind::SessionUrl)
        .and_then(|url| {
            url.strip_prefix("https://")?
                .strip_suffix("/.well-known/jmap")
        })
        .map(str::to_owned)
        .or_else(|| prefill(FieldKind::OutgoingServer).and_then(|h| after_label(h, &["smtp"])))
        .or_else(|| prefill(FieldKind::Server).and_then(|h| after_label(h, &GUESS_LABELS)))
}

/// The server form as `answers` shape it: the protocol decides which fields there are, and a
/// port that is empty or one of the usual ones follows protocol and security (one the person
/// typed otherwise stays). Everything else typed stays as the prefill. A form that is not the
/// server form is returned as it is.
pub fn refit(fields: &[FieldSpec], answers: &[FieldAnswer]) -> Vec<FieldSpec> {
    refit_in(fields, answers, PROTOCOL_CHOICES)
}

fn refit_in(fields: &[FieldSpec], answers: &[FieldAnswer], choices: &[&str]) -> Vec<FieldSpec> {
    if !is_manual(fields) {
        return fields.to_vec();
    }
    let protocol = text(answers, FieldKind::Protocol)
        .and_then(|t| Protocol::offered(&t, choices))
        .or_else(|| {
            let was = fields.iter().find(|f| f.kind == FieldKind::Protocol)?;
            Protocol::offered(was.prefill.as_deref()?, choices)
        })
        .unwrap_or(Protocol::Imap);
    let security = |kind| {
        text(answers, kind)
            .and_then(|t| Security::parse(&t))
            .unwrap_or(Security::Tls)
    };
    let kept = |kind: FieldKind, base: Option<String>| {
        text(answers, kind).or(base).filter(|t| !t.is_empty())
    };
    let domain = domain_hint(fields);
    manual_form(protocol, domain.as_deref())
        .into_iter()
        .map(|mut field| {
            let original = fields.iter().find(|f| f.kind == field.kind);
            // What the form had, else the guess the domain gives this protocol's field.
            let base = original
                .and_then(|f| f.prefill.clone())
                .or_else(|| field.prefill.clone());
            field.prefill = match field.kind {
                FieldKind::Protocol => Some(protocol.slug().to_owned()),
                FieldKind::Security | FieldKind::OutgoingSecurity => {
                    Some(security(field.kind).slug().to_owned())
                }
                FieldKind::Port | FieldKind::OutgoingPort => {
                    let outgoing = field.kind == FieldKind::OutgoingPort;
                    let kind = match outgoing {
                        true => FieldKind::OutgoingSecurity,
                        false => FieldKind::Security,
                    };
                    let usual = usual_port(protocol, outgoing, security(kind)).to_string();
                    let typed = text(answers, field.kind);
                    let own = typed
                        .filter(|t| t.parse::<u16>().map_or(true, |p| !USUAL_PORTS.contains(&p)));
                    Some(own.unwrap_or(usual))
                }
                FieldKind::Server => kept(field.kind, base).map(|host| retarget(&host, protocol)),
                // A secret is never carried into the form that is shown again.
                FieldKind::Token => None,
                _ => kept(field.kind, base),
            };
            field
        })
        .collect()
}

fn invalid(field: FieldKind) -> FieldProblem {
    FieldProblem {
        field,
        problem: ProblemKind::Invalid,
    }
}

fn missing(field: FieldKind) -> FieldProblem {
    FieldProblem {
        field,
        problem: ProblemKind::Missing,
    }
}

/// A host name or address: letters, digits, dots, hyphens, underscores; no scheme, port, path,
/// user info or white space; not starting with a dot or a hyphen.
fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['.', '-'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn hop(answers: &[FieldAnswer], protocol: Protocol, outgoing: bool) -> Result<Hop, FieldProblem> {
    let (host_kind, security_kind, port_kind) = match outgoing {
        false => (FieldKind::Server, FieldKind::Security, FieldKind::Port),
        true => (
            FieldKind::OutgoingServer,
            FieldKind::OutgoingSecurity,
            FieldKind::OutgoingPort,
        ),
    };
    let host = text(answers, host_kind).ok_or(missing(host_kind))?;
    if !host_ok(&host) {
        return Err(invalid(host_kind));
    }
    let security = text(answers, security_kind)
        .ok_or(missing(security_kind))
        .and_then(|t| Security::parse(&t).ok_or(invalid(security_kind)))?;
    if security == Security::Plain && !is_loopback(&host) {
        return Err(invalid(security_kind));
    }
    let port = match text(answers, port_kind) {
        None => usual_port(protocol, outgoing, security),
        Some(t) => t
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| t.parse::<u16>().ok().filter(|p| *p != 0))
            .flatten()
            .ok_or(invalid(port_kind))?,
    };
    Ok(Hop {
        host,
        port,
        security,
    })
}

fn session_url(answers: &[FieldAnswer]) -> Result<EndpointUrl, FieldProblem> {
    let typed = text(answers, FieldKind::SessionUrl).ok_or(missing(FieldKind::SessionUrl))?;
    let url = EndpointUrl::parse(&typed).map_err(|_| invalid(FieldKind::SessionUrl))?;
    let origin = url.origin();
    match origin.scheme {
        UrlScheme::Https => Ok(url),
        UrlScheme::Http if origin.is_loopback() => Ok(url),
        _ => Err(invalid(FieldKind::SessionUrl)),
    }
}

fn login(answers: &[FieldAnswer]) -> Result<Option<String>, FieldProblem> {
    match text(answers, FieldKind::Username) {
        None => Ok(None),
        Some(name) if name.len() <= 256 && !name.chars().any(char::is_control) => Ok(Some(name)),
        Some(_) => Err(invalid(FieldKind::Username)),
    }
}

/// What the answers to the server form mean, or the first field (in form order) that is missing
/// or cannot be right. An empty port is the usual one; an empty login name is the address.
pub fn parse_manual(answers: &[FieldAnswer]) -> Result<Manual, FieldProblem> {
    parse_in(answers, PROTOCOL_CHOICES)
}

fn parse_in(answers: &[FieldAnswer], choices: &[&str]) -> Result<Manual, FieldProblem> {
    let typed = text(answers, FieldKind::Protocol).ok_or(missing(FieldKind::Protocol))?;
    let protocol = Protocol::offered(&typed, choices).ok_or(invalid(FieldKind::Protocol))?;
    match protocol {
        Protocol::Jmap => {
            let session = session_url(answers)?;
            let login = login(answers)?;
            let token = secret(answers, FieldKind::Token);
            Ok(Manual::Jmap(JmapServer {
                session,
                login,
                token,
            }))
        }
        _ => {
            let incoming = hop(answers, protocol, false)?;
            let outgoing = hop(answers, protocol, true)?;
            let login = login(answers)?;
            let servers = MailServers {
                incoming,
                outgoing,
                login,
            };
            Ok(match protocol {
                Protocol::Pop3 => Manual::Pop3(servers),
                _ => Manual::Imap(servers),
            })
        }
    }
}

/// What is wrong with a form's answers, if anything: the first required field left empty, then,
/// for the server form, the first answer that cannot be right. A host keeps Continue off while
/// this is `Some`, and marks the field; the sheet's machine checks the same before it sends.
pub fn form_problem(fields: &[FieldSpec], answers: &[FieldAnswer]) -> Option<FieldProblem> {
    if let Some(field) = first_missing(fields, answers) {
        return Some(missing(field));
    }
    match is_manual(fields) {
        true => parse_manual(answers).err(),
        false => None,
    }
}

#[cfg(test)]
mod tests;
