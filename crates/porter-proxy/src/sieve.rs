//! The ManageSieve relay (RFC 5804), for the sieve client of mailo's mail runtime. The server
//! opens with a capability list; the relay upgrades with `STARTTLS` when the endpoint says so
//! (the server announces its capabilities again after the handshake), authenticates with
//! `AUTHENTICATE` and the initial response, and gives the app the capability list without
//! `SASL` and `STARTTLS` followed by `OK`, then relays bytes.

use crate::fault::RelayFault;
use crate::lines::{MAX_EARLY, closes, fail, finished, overlong, send, send_line, take_line, text};
use crate::step::{Effect, Input, Relaying, Side};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::{RelayAuth, RelayPlan, Tls};

/// How the relay authenticates to a ManageSieve server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SieveAuth {
    /// `AUTHENTICATE "PLAIN"`.
    Plain,
    /// `AUTHENTICATE "XOAUTH2"`.
    Xoauth2,
}

/// The name of a capability line (`"SASL" "PLAIN"` is `SASL`), upper case.
fn capability_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix('"')?;
    let (name, _) = rest.split_once('"')?;
    Some(name.to_ascii_uppercase())
}

/// The value of a capability line (`PLAIN LOGIN` for `"SASL" "PLAIN LOGIN"`).
fn capability_value(line: &str) -> Option<String> {
    let rest = line.strip_prefix('"')?.split_once('"')?.1.trim();
    let value = rest.strip_prefix('"')?.rsplit_once('"')?.0;
    Some(value.to_owned())
}

/// The mechanisms the `SASL` capability line offers, upper case.
fn mechanisms(capabilities: &[String]) -> Vec<String> {
    capabilities
        .iter()
        .filter(|l| capability_name(l).as_deref() == Some("SASL"))
        .filter_map(|l| capability_value(l))
        .flat_map(|v| {
            v.split(' ')
                .map(str::to_ascii_uppercase)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn has_capability(capabilities: &[String], name: &str) -> bool {
    capabilities
        .iter()
        .any(|l| capability_name(l).as_deref() == Some(name))
}

impl SieveAuth {
    /// The way to present `auth` to a server whose capability lines are `capabilities`, or
    /// `Protocol` when it offers none: a password needs `PLAIN`, a token `XOAUTH2`.
    pub fn choose(capabilities: &[String], auth: &RelayAuth) -> Result<Self, RelayFault> {
        let offered = mechanisms(capabilities);
        let has = |name: &str| offered.iter().any(|m| m == name);
        match auth {
            RelayAuth::Password(_) if has("PLAIN") => Ok(SieveAuth::Plain),
            RelayAuth::AccessToken(_) if has("XOAUTH2") => Ok(SieveAuth::Xoauth2),
            RelayAuth::Password(_) | RelayAuth::AccessToken(_) => Err(RelayFault::Protocol),
        }
    }
}

/// What the app is told on connecting: the server's capability lines less `SASL` and
/// `STARTTLS`, then `OK`.
pub fn app_greeting(capabilities: &[String]) -> Vec<u8> {
    let mut out: String = capabilities
        .iter()
        .filter(|l| !matches!(capability_name(l).as_deref(), Some("SASL" | "STARTTLS")))
        .map(|l| format!("{l}\r\n"))
        .collect();
    out.push_str("OK \"porter relay ready\"\r\n");
    out.into_bytes()
}

/// Where the relay stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SievePhase {
    /// Reading a capability list (the greeting, and again after a `STARTTLS` upgrade).
    Capability,
    /// Waiting for the answer to `STARTTLS`, then the upgrade.
    StartTls,
    /// `AUTHENTICATE` is sent; waiting for the answer.
    Authenticating,
    /// Relaying bytes both ways.
    Relaying,
}

/// One ManageSieve relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SieveRelay {
    /// What it presents and where.
    pub plan: RelayPlan,
    /// Where it stands.
    pub phase: SievePhase,
    /// Server bytes read and not yet a whole line, until the relay starts relaying.
    pub buffer: Vec<u8>,
    /// What the app sent before the relay was ready for it.
    pub early: Vec<u8>,
    /// The capability lines of the list being read, or last read.
    pub capabilities: Vec<String>,
}

impl SieveRelay {
    /// A relay at the start of a connection.
    pub fn new(plan: RelayPlan) -> Self {
        Self {
            plan,
            phase: SievePhase::Capability,
            buffer: Vec::new(),
            early: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    fn authenticate(&mut self) -> Vec<Effect> {
        let auth = match SieveAuth::choose(&self.capabilities, &self.plan.auth) {
            Ok(auth) => auth,
            Err(fault) => return fail(fault),
        };
        let user = &self.plan.endpoint.login.0;
        let (name, response) = match (auth, &self.plan.auth) {
            (SieveAuth::Plain, RelayAuth::Password(password)) => {
                ("PLAIN", format!("\0{user}\0{}", password.expose()))
            }
            (SieveAuth::Xoauth2, RelayAuth::AccessToken(token)) => (
                "XOAUTH2",
                format!("user={user}\x01auth=Bearer {}\x01\x01", token.expose()),
            ),
            _ => return fail(RelayFault::Protocol),
        };
        self.phase = SievePhase::Authenticating;
        vec![send_line(
            Side::Server,
            &format!("AUTHENTICATE \"{name}\" \"{}\"", STANDARD.encode(response)),
        )]
    }

    /// A complete capability list is in.
    fn listed(&mut self) -> Vec<Effect> {
        match self.plan.endpoint.tls {
            Tls::StartTls if has_capability(&self.capabilities, "STARTTLS") => {
                self.phase = SievePhase::StartTls;
                vec![send_line(Side::Server, "STARTTLS")]
            }
            // A credential is never sent before the connection is secure.
            Tls::StartTls => fail(RelayFault::Protocol),
            Tls::Implicit | Tls::Plain => self.authenticate(),
        }
    }

    fn start_relaying(&mut self) -> Vec<Effect> {
        self.phase = SievePhase::Relaying;
        let mut effects = vec![send(Side::App, &app_greeting(&self.capabilities))];
        let held = std::mem::take(&mut self.buffer);
        if !held.is_empty() {
            effects.push(send(Side::App, &held));
        }
        let early = std::mem::take(&mut self.early);
        if !early.is_empty() {
            effects.push(send(Side::Server, &early));
        }
        effects
    }

    fn on_line(&mut self, line: &str) -> Vec<Effect> {
        let status = line
            .split([' ', '('])
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let is_status = !line.starts_with('"') && matches!(status.as_str(), "OK" | "NO" | "BYE");
        if status == "BYE" && is_status {
            return fail(RelayFault::Unreachable);
        }
        match (self.phase, is_status, status.as_str()) {
            (SievePhase::Capability, false, _) => {
                self.capabilities.push(line.to_owned());
                Vec::new()
            }
            (SievePhase::Capability, true, "OK") => self.listed(),
            (SievePhase::StartTls, true, "OK") => match self.buffer.is_empty() {
                // Bytes after the `OK` and before the handshake would be read as TLS.
                true => vec![Effect::StartTls],
                false => fail(RelayFault::Protocol),
            },
            (SievePhase::Authenticating, true, "OK") => self.start_relaying(),
            (SievePhase::Authenticating, true, "NO") => fail(RelayFault::Refused),
            // An error challenge: an empty string gets the real refusal.
            (SievePhase::Authenticating, false, _) => vec![send_line(Side::Server, "\"\"")],
            _ => fail(RelayFault::Protocol),
        }
    }

    fn on_server_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        if self.phase == SievePhase::Relaying {
            return vec![send(Side::App, data)];
        }
        self.buffer.extend_from_slice(data);
        let mut effects = Vec::new();
        while let Some(line) = take_line(&mut self.buffer) {
            effects.extend(self.on_line(&text(&line)));
            if closes(&effects)
                || self.phase == SievePhase::Relaying
                || effects.contains(&Effect::StartTls)
            {
                return effects;
            }
        }
        match overlong(&self.buffer) {
            true => fail(RelayFault::Protocol),
            false => effects,
        }
    }

    fn on_app_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        if self.phase == SievePhase::Relaying {
            return vec![send(Side::Server, data)];
        }
        self.early.extend_from_slice(data);
        match self.early.len() > MAX_EARLY {
            true => fail(RelayFault::Protocol),
            false => Vec::new(),
        }
    }
}

impl Relaying for SieveRelay {
    fn step(mut self, input: Input) -> (Self, Vec<Effect>) {
        let effects = match input {
            Input::Start => Vec::new(),
            Input::TlsReady if self.phase == SievePhase::StartTls => {
                // From here the connection is TLS, as if the endpoint had said so. The server
                // announces its capabilities again (RFC 5804 2.2).
                self.plan.endpoint.tls = Tls::Implicit;
                self.capabilities.clear();
                self.phase = SievePhase::Capability;
                Vec::new()
            }
            Input::TlsReady => fail(RelayFault::Protocol),
            Input::Closed(Side::App) => finished(),
            Input::Closed(Side::Server) => match self.phase {
                SievePhase::Relaying => finished(),
                _ => fail(RelayFault::Protocol),
            },
            Input::Bytes {
                from: Side::Server,
                data,
            } => self.on_server_bytes(&data),
            Input::Bytes {
                from: Side::App,
                data,
            } => self.on_app_bytes(&data),
        };
        (self, effects)
    }
}

#[cfg(test)]
mod tests;
