//! The IMAP relay's steps. Tags are fixed per phase (`P1` .. `P5`): the relay has one command
//! in flight at a time, so a tag only tells an answer from the untagged lines around it.

use super::{ImapAuth, ImapPhase, ImapRelay, preauth_greeting};
use crate::fault::RelayFault;
use crate::lines::{MAX_EARLY, closes, fail, finished, overlong, send, send_line, take_line, text};
use crate::step::{Effect, Input, Relaying, Side};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::{RelayAuth, Tls};

const CAPABILITY: &str = "P1";
const STARTTLS: &str = "P2";
const CAPABILITY_SECURE: &str = "P3";
const AUTH: &str = "P4";
const CAPABILITY_AUTHED: &str = "P5";

/// What a tagged line says: its status word, if it carries `tag`.
fn tagged<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(tag)?.strip_prefix(' ')?;
    rest.split(' ').next()
}

/// The capability atoms of an untagged `* CAPABILITY ...` line, or of a `[CAPABILITY ...]`
/// response code anywhere in a line.
fn capabilities_of(line: &str) -> Option<Vec<String>> {
    let upper = line.to_ascii_uppercase();
    let atoms = match upper.strip_prefix("* CAPABILITY ") {
        Some(_) => &line["* CAPABILITY ".len()..],
        None => {
            let at = upper.find("[CAPABILITY ")? + "[CAPABILITY ".len();
            let end = line[at..].find(']')?;
            &line[at..at + end]
        }
    };
    Some(atoms.split_whitespace().map(str::to_owned).collect())
}

/// `text` as an IMAP quoted string, or `None` when it cannot be one (a line break or NUL).
fn quoted(text: &str) -> Option<String> {
    (!text.contains(['\r', '\n', '\0'])).then(|| {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    })
}

impl ImapRelay {
    fn secret_line(&self, auth: ImapAuth) -> Option<String> {
        let user = &self.plan.endpoint.login.0;
        match (&self.plan.auth, auth) {
            (RelayAuth::Password(password), ImapAuth::Plain) => {
                Some(STANDARD.encode(format!("\0{user}\0{}", password.expose())))
            }
            (RelayAuth::AccessToken(token), ImapAuth::Xoauth2) => Some(STANDARD.encode(format!(
                "user={user}\x01auth=Bearer {}\x01\x01",
                token.expose()
            ))),
            _ => None,
        }
    }

    /// The server's capabilities are known: upgrade first if the endpoint says so, else log in.
    fn proceed(&mut self) -> Vec<Effect> {
        let has = |atom: &str| {
            self.capabilities
                .iter()
                .any(|c| c.eq_ignore_ascii_case(atom))
        };
        match self.plan.endpoint.tls {
            Tls::StartTls if has("STARTTLS") => {
                self.phase = ImapPhase::StartTls;
                vec![send_line(Side::Server, &format!("{STARTTLS} STARTTLS"))]
            }
            // A credential is never sent before the connection is secure.
            Tls::StartTls => fail(RelayFault::Protocol),
            Tls::Implicit | Tls::Plain => self.authenticate(),
        }
    }

    fn authenticate(&mut self) -> Vec<Effect> {
        let auth = match ImapAuth::choose(&self.capabilities, &self.plan.auth) {
            Ok(auth) => auth,
            Err(fault) => return fail(fault),
        };
        let line = match (auth, &self.plan.auth) {
            (ImapAuth::Login, RelayAuth::Password(password)) => {
                match (
                    quoted(&self.plan.endpoint.login.0),
                    quoted(password.expose()),
                ) {
                    (Some(user), Some(password)) => format!("{AUTH} LOGIN {user} {password}"),
                    _ => return fail(RelayFault::Protocol),
                }
            }
            (ImapAuth::Plain, _) => format!("{AUTH} AUTHENTICATE PLAIN"),
            (ImapAuth::Xoauth2, _) => format!("{AUTH} AUTHENTICATE XOAUTH2"),
            // A token or no credential cannot log in, and neither can a way a newer porter adds.
            (ImapAuth::Login, _) => {
                return fail(RelayFault::Protocol);
            }
        };
        self.phase = ImapPhase::Authenticating(auth);
        vec![send_line(Side::Server, &line)]
    }

    /// The session is authenticated: tell the app, and pass on what it sent early.
    fn start_relaying(&mut self) -> Vec<Effect> {
        self.phase = ImapPhase::Relaying;
        let mut effects = vec![send(Side::App, &preauth_greeting(&self.capabilities))];
        let held: Vec<u8> = std::mem::take(&mut self.buffer);
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
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("* BYE") {
            return fail(RelayFault::Unreachable);
        }
        match self.phase.clone() {
            ImapPhase::Greeting => {
                if !upper.starts_with("* OK") {
                    // `* PREAUTH` has nothing for the relay to do, and anything else is not IMAP.
                    return fail(RelayFault::Protocol);
                }
                match capabilities_of(line) {
                    Some(caps) => {
                        self.capabilities = caps;
                        self.proceed()
                    }
                    None => {
                        self.phase = ImapPhase::Capability;
                        vec![send_line(Side::Server, &format!("{CAPABILITY} CAPABILITY"))]
                    }
                }
            }
            ImapPhase::Capability => {
                if let Some(caps) = capabilities_of(line).filter(|_| upper.starts_with('*')) {
                    self.capabilities = caps;
                    return Vec::new();
                }
                match tagged(line, CAPABILITY).or_else(|| tagged(line, CAPABILITY_SECURE)) {
                    Some(status) if status.eq_ignore_ascii_case("OK") => self.proceed(),
                    Some(_) => fail(RelayFault::Protocol),
                    None => Vec::new(),
                }
            }
            ImapPhase::StartTls => match tagged(line, STARTTLS) {
                Some(status) if status.eq_ignore_ascii_case("OK") => {
                    // Bytes after the `OK` and before the handshake would be read as TLS.
                    match self.buffer.is_empty() {
                        true => vec![Effect::StartTls],
                        false => fail(RelayFault::Protocol),
                    }
                }
                Some(_) => fail(RelayFault::Protocol),
                None => Vec::new(),
            },
            ImapPhase::Authenticating(auth) => self.on_auth_line(line, Some(auth)),
            ImapPhase::Answering => self.on_auth_line(line, None),
            ImapPhase::PostAuthCapability => {
                if let Some(caps) = capabilities_of(line).filter(|_| upper.starts_with('*')) {
                    self.capabilities = caps;
                    return Vec::new();
                }
                match tagged(line, CAPABILITY_AUTHED) {
                    // A server that will not repeat them keeps the ones it announced.
                    Some(_) => self.start_relaying(),
                    None => Vec::new(),
                }
            }
            ImapPhase::Relaying => Vec::new(),
        }
    }

    /// A line while authenticating: `unanswered` is the way when no response has been sent.
    fn on_auth_line(&mut self, line: &str, unanswered: Option<ImapAuth>) -> Vec<Effect> {
        if line.starts_with('+') {
            return match unanswered.and_then(|auth| self.secret_line(auth)) {
                Some(response) => {
                    self.phase = ImapPhase::Answering;
                    vec![send_line(Side::Server, &response)]
                }
                None if unanswered.is_none() => vec![send_line(Side::Server, "")],
                None => fail(RelayFault::Protocol),
            };
        }
        match tagged(line, AUTH) {
            Some(status) if status.eq_ignore_ascii_case("OK") => match capabilities_of(line) {
                Some(caps) => {
                    self.capabilities = caps;
                    self.start_relaying()
                }
                None => {
                    self.phase = ImapPhase::PostAuthCapability;
                    vec![send_line(
                        Side::Server,
                        &format!("{CAPABILITY_AUTHED} CAPABILITY"),
                    )]
                }
            },
            Some(status) if status.eq_ignore_ascii_case("NO") => fail(RelayFault::Refused),
            Some(_) => fail(RelayFault::Protocol),
            None => Vec::new(),
        }
    }

    fn on_server_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        if self.phase == ImapPhase::Relaying {
            return vec![send(Side::App, data)];
        }
        self.buffer.extend_from_slice(data);
        let mut effects = Vec::new();
        while let Some(line) = take_line(&mut self.buffer) {
            effects.extend(self.on_line(&text(&line)));
            if closes(&effects)
                || self.phase == ImapPhase::Relaying
                || effects.contains(&Effect::StartTls)
            {
                return effects;
            }
        }
        if overlong(&self.buffer) {
            return fail(RelayFault::Protocol);
        }
        effects
    }

    fn on_app_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        if self.phase == ImapPhase::Relaying {
            return vec![send(Side::Server, data)];
        }
        self.early.extend_from_slice(data);
        match self.early.len() > MAX_EARLY {
            true => fail(RelayFault::Protocol),
            false => Vec::new(),
        }
    }
}

impl Relaying for ImapRelay {
    fn step(mut self, input: Input) -> (Self, Vec<Effect>) {
        let effects = match input {
            Input::Start => Vec::new(),
            Input::TlsReady if self.phase == ImapPhase::StartTls => {
                // From here the connection is TLS, as if the endpoint had said so.
                self.plan.endpoint.tls = Tls::Implicit;
                self.capabilities.clear();
                self.phase = ImapPhase::Capability;
                vec![send_line(
                    Side::Server,
                    &format!("{CAPABILITY_SECURE} CAPABILITY"),
                )]
            }
            Input::TlsReady => fail(RelayFault::Protocol),
            Input::Closed(Side::App) => finished(),
            Input::Closed(Side::Server) => match self.phase {
                ImapPhase::Relaying => finished(),
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
