//! The POP3 relay's steps: the handshake with the server, then a command-aware relay. The app's
//! commands are read as lines so that its login commands are never forwarded; a server reply
//! (a message is a long one) passes to the app unread.

use super::{Pop3Auth, Pop3Phase, Pop3Relay, app_greeting};
use crate::fault::RelayFault;
use crate::lines::{MAX_EARLY, closes, fail, finished, overlong, send, send_line, take_line, text};
use crate::step::{Effect, Input, Relaying, Side};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::{RelayAuth, Tls};

fn ok(line: &str) -> bool {
    line == "+OK" || line.starts_with("+OK ")
}

fn err(line: &str) -> bool {
    line == "-ERR" || line.starts_with("-ERR")
}

/// A refusal that is the mailbox's state, not the credential's (`[IN-USE]`, `[LOGIN-DELAY]`).
fn busy(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    upper.contains("[IN-USE]") || upper.contains("[LOGIN-DELAY]")
}

fn refused(line: &str) -> Vec<Effect> {
    match busy(line) {
        true => fail(RelayFault::Unreachable),
        false => fail(RelayFault::Refused),
    }
}

impl Pop3Relay {
    fn authenticate(&mut self) -> Vec<Effect> {
        let auth = match Pop3Auth::choose(&self.capabilities, &self.plan.auth) {
            Ok(auth) => auth,
            Err(fault) => return fail(fault),
        };
        let user = self.plan.endpoint.login.0.clone();
        let line = match (auth, &self.plan.auth) {
            (Pop3Auth::User, RelayAuth::Password(_)) => {
                self.phase = Pop3Phase::User;
                format!("USER {user}")
            }
            (Pop3Auth::Plain, RelayAuth::Password(password)) => {
                self.phase = Pop3Phase::Authenticating;
                format!(
                    "AUTH PLAIN {}",
                    STANDARD.encode(format!("\0{user}\0{}", password.expose()))
                )
            }
            (Pop3Auth::Xoauth2, RelayAuth::AccessToken(token)) => {
                self.phase = Pop3Phase::Authenticating;
                format!(
                    "AUTH XOAUTH2 {}",
                    STANDARD.encode(format!(
                        "user={user}\x01auth=Bearer {}\x01\x01",
                        token.expose()
                    ))
                )
            }
            _ => return fail(RelayFault::Protocol),
        };
        vec![send_line(Side::Server, &line)]
    }

    /// The capabilities are known: upgrade, or authenticate.
    fn after_capabilities(&mut self) -> Vec<Effect> {
        let first = self.phase == Pop3Phase::Capability;
        match (self.plan.endpoint.tls, first) {
            (Tls::StartTls, true)
                if self
                    .capabilities
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case("STLS")) =>
            {
                self.phase = Pop3Phase::StartTls;
                vec![send_line(Side::Server, "STLS")]
            }
            // A credential is never sent before the connection is secure.
            (Tls::StartTls, true) => fail(RelayFault::Protocol),
            _ => self.authenticate(),
        }
    }

    /// One line of a `CAPA` reply; the effects once it is complete.
    fn on_capability_line(&mut self, line: &str) -> Vec<Effect> {
        match self.reply.take() {
            None if ok(line) => {
                self.reply = Some(Vec::new());
                Vec::new()
            }
            // A server without `CAPA`: no capabilities.
            None if err(line) => {
                self.capabilities.clear();
                self.after_capabilities()
            }
            None => fail(RelayFault::Protocol),
            Some(lines) if line == "." => {
                self.capabilities = lines;
                self.after_capabilities()
            }
            Some(mut lines) => {
                lines.push(line.strip_prefix('.').unwrap_or(line).to_owned());
                self.reply = Some(lines);
                Vec::new()
            }
        }
    }

    fn ready(&mut self) -> Vec<Effect> {
        self.phase = Pop3Phase::Relaying;
        let mut effects = vec![send(Side::App, &app_greeting())];
        let early = std::mem::take(&mut self.early);
        effects.extend(self.on_app_bytes(&early));
        effects
    }

    fn on_line(&mut self, line: &str) -> Vec<Effect> {
        match self.phase.clone() {
            Pop3Phase::Greeting => match ok(line) {
                true => {
                    self.phase = Pop3Phase::Capability;
                    vec![send_line(Side::Server, "CAPA")]
                }
                false => fail(RelayFault::Protocol),
            },
            Pop3Phase::Capability | Pop3Phase::CapabilityAgain => self.on_capability_line(line),
            Pop3Phase::StartTls => match (ok(line), self.buffer.is_empty()) {
                // Bytes after the `+OK` and before the handshake would be read as TLS.
                (true, true) => vec![Effect::StartTls],
                _ => fail(RelayFault::Protocol),
            },
            Pop3Phase::User => match (ok(line), err(line)) {
                (true, _) => {
                    let RelayAuth::Password(password) = &self.plan.auth else {
                        return fail(RelayFault::Protocol);
                    };
                    let pass = format!("PASS {}", password.expose());
                    self.phase = Pop3Phase::Pass;
                    vec![send_line(Side::Server, &pass)]
                }
                (_, true) => refused(line),
                _ => fail(RelayFault::Protocol),
            },
            Pop3Phase::Pass => match (ok(line), err(line)) {
                (true, _) => self.ready(),
                (_, true) => refused(line),
                _ => fail(RelayFault::Protocol),
            },
            Pop3Phase::Authenticating => match (ok(line), err(line)) {
                (true, _) => self.ready(),
                (_, true) => refused(line),
                // XOAUTH2's error challenge: an empty line gets the real refusal.
                _ if line.starts_with('+') => vec![send_line(Side::Server, "")],
                _ => fail(RelayFault::Protocol),
            },
            Pop3Phase::Relaying => Vec::new(),
        }
    }

    fn on_server_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        if self.phase == Pop3Phase::Relaying {
            return vec![send(Side::App, data)];
        }
        self.buffer.extend_from_slice(data);
        let mut effects = Vec::new();
        while let Some(line) = take_line(&mut self.buffer) {
            effects.extend(self.on_line(&text(&line)));
            if closes(&effects) || effects.contains(&Effect::StartTls) {
                return effects;
            }
            if self.phase == Pop3Phase::Relaying {
                // What the server sent after its last reply is the app's.
                let rest = std::mem::take(&mut self.buffer);
                if !rest.is_empty() {
                    effects.push(send(Side::App, &rest));
                }
                return effects;
            }
        }
        match overlong(&self.buffer) {
            true => fail(RelayFault::Protocol),
            false => effects,
        }
    }

    fn on_app_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        self.early.extend_from_slice(data);
        if self.phase != Pop3Phase::Relaying {
            return match self.early.len() > MAX_EARLY {
                true => fail(RelayFault::Protocol),
                false => Vec::new(),
            };
        }
        let mut effects = Vec::new();
        while let Some(line) = take_line(&mut self.early) {
            effects.extend(command(&text(&line), &line));
        }
        match overlong(&self.early) {
            true => fail(RelayFault::Protocol),
            false => effects,
        }
    }
}

/// What one command line of the app comes to.
fn command(line: &str, raw: &[u8]) -> Vec<Effect> {
    let verb = line
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    match verb.as_str() {
        // The relay has logged in: a client that logs in anyway is told it worked.
        "USER" | "PASS" | "APOP" => vec![send_line(Side::App, "+OK already authenticated")],
        "AUTH" => vec![send_line(Side::App, "-ERR already authenticated")],
        "STLS" => vec![send_line(Side::App, "-ERR TLS is already active")],
        _ => {
            let mut data = raw.to_vec();
            data.extend_from_slice(b"\r\n");
            vec![Effect::Send {
                to: Side::Server,
                data,
            }]
        }
    }
}

impl Relaying for Pop3Relay {
    fn step(mut self, input: Input) -> (Self, Vec<Effect>) {
        let effects = match input {
            Input::Start => Vec::new(),
            Input::TlsReady if self.phase == Pop3Phase::StartTls => {
                // From here the connection is TLS, as if the endpoint had said so.
                self.plan.endpoint.tls = Tls::Implicit;
                self.phase = Pop3Phase::CapabilityAgain;
                self.reply = None;
                vec![send_line(Side::Server, "CAPA")]
            }
            Input::TlsReady => fail(RelayFault::Protocol),
            Input::Closed(Side::App) => finished(),
            Input::Closed(Side::Server) => match self.phase {
                Pop3Phase::Relaying => finished(),
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
