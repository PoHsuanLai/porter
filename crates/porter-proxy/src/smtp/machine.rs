//! The SMTP relay's steps: the handshake with the server, then a command-aware relay. The app's
//! commands are read as lines so that `AUTH` and `STARTTLS` are never forwarded, and a message
//! body (after the server's `354`) or a `BDAT` chunk is passed through unread, so a body line
//! that looks like a command is only text.

use super::{EhloReply, SmtpAuth, SmtpPhase, SmtpRelay, app_greeting};
use crate::fault::RelayFault;
use crate::lines::{MAX_EARLY, closes, fail, finished, overlong, send, send_line, take_line, text};
use crate::step::{Effect, Input, Relaying, Side};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::{RelayAuth, Tls};

/// The name the relay gives the server in `EHLO`.
const EHLO_NAME: &str = "localhost";
/// How a body ends.
const END_OF_DATA: &[u8] = b"\r\n.\r\n";

/// The code of a reply line, and whether it is the last line of its reply.
fn code_of(line: &str) -> Option<(u16, bool)> {
    let (code, rest) = line.split_at_checked(3)?;
    let code: u16 = code.parse().ok()?;
    match rest.chars().next() {
        None | Some(' ') => Some((code, true)),
        Some('-') => Some((code, false)),
        Some(_) => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Whether the server's bytes hold a line that starts with `354`, tracking the line across
/// reads in `head`.
fn saw_go_ahead(head: &mut Vec<u8>, data: &[u8]) -> bool {
    let mut seen = false;
    for &byte in data {
        match byte {
            b'\n' => {
                let go =
                    head.starts_with(b"354") && matches!(head.get(3), None | Some(b' ' | b'\r'));
                seen |= go;
                head.clear();
            }
            _ if head.len() < 4 => head.push(byte),
            _ => {}
        }
    }
    seen
}

impl SmtpRelay {
    fn authenticate(&mut self) -> Vec<Effect> {
        let Some(ehlo) = self.ehlo.clone() else {
            return fail(RelayFault::Protocol);
        };
        let auth = match SmtpAuth::choose(&ehlo, &self.plan.auth) {
            Ok(auth) => auth,
            Err(fault) => return fail(fault),
        };
        let user = &self.plan.endpoint.login.0;
        let line = match (auth, &self.plan.auth) {
            (SmtpAuth::Plain, RelayAuth::Password(password)) => format!(
                "AUTH PLAIN {}",
                STANDARD.encode(format!("\0{user}\0{}", password.expose()))
            ),
            (SmtpAuth::Xoauth2, RelayAuth::AccessToken(token)) => format!(
                "AUTH XOAUTH2 {}",
                STANDARD.encode(format!(
                    "user={user}\x01auth=Bearer {}\x01\x01",
                    token.expose()
                ))
            ),
            _ => return fail(RelayFault::Protocol),
        };
        self.phase = SmtpPhase::Authenticating(auth);
        vec![send_line(Side::Server, &line)]
    }

    /// A complete reply from the server, as `(code, lines)`.
    fn on_reply(&mut self, code: u16, reply: Vec<u8>) -> Vec<Effect> {
        match self.phase.clone() {
            SmtpPhase::Greeting => match code {
                220 => {
                    self.phase = SmtpPhase::Ehlo;
                    vec![send_line(Side::Server, &format!("EHLO {EHLO_NAME}"))]
                }
                _ => fail(RelayFault::Protocol),
            },
            SmtpPhase::Ehlo | SmtpPhase::EhloAgain => {
                let ehlo = match EhloReply::parse(&reply) {
                    Ok(ehlo) if code == 250 => ehlo,
                    _ => return fail(RelayFault::Protocol),
                };
                let first = self.phase == SmtpPhase::Ehlo;
                self.ehlo = Some(ehlo.clone());
                match (self.plan.endpoint.tls, first) {
                    (Tls::StartTls, true) if ehlo.offers("STARTTLS") => {
                        self.phase = SmtpPhase::StartTls;
                        vec![send_line(Side::Server, "STARTTLS")]
                    }
                    // A credential is never sent before the connection is secure.
                    (Tls::StartTls, true) => fail(RelayFault::Protocol),
                    _ => self.authenticate(),
                }
            }
            SmtpPhase::StartTls => match (code, self.buffer.is_empty()) {
                // Bytes after the `220` and before the handshake would be read as TLS.
                (220, true) => vec![Effect::StartTls],
                _ => fail(RelayFault::Protocol),
            },
            SmtpPhase::Authenticating(_) => match code {
                235 => match self.ehlo.clone() {
                    Some(ehlo) => {
                        self.phase = SmtpPhase::AppEhlo(ehlo);
                        let mut effects = vec![send(Side::App, &app_greeting())];
                        let early = std::mem::take(&mut self.early);
                        effects.extend(self.on_app_bytes(&early));
                        effects
                    }
                    None => fail(RelayFault::Protocol),
                },
                // XOAUTH2's error challenge: an empty line gets the real refusal.
                334 => vec![send_line(Side::Server, "")],
                530 | 534 | 535 | 538 => fail(RelayFault::Refused),
                _ => fail(RelayFault::Protocol),
            },
            SmtpPhase::AppEhlo(_)
            | SmtpPhase::Relaying
            | SmtpPhase::Data
            | SmtpPhase::Chunk { .. } => Vec::new(),
        }
    }

    fn on_server_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        let handshaking = !matches!(
            self.phase,
            SmtpPhase::AppEhlo(_) | SmtpPhase::Relaying | SmtpPhase::Data | SmtpPhase::Chunk { .. }
        );
        if !handshaking {
            // Everything the server says reaches the app; a `354` also starts a body.
            if self.phase == SmtpPhase::Relaying && saw_go_ahead(&mut self.head, data) {
                self.phase = SmtpPhase::Data;
                self.tail = b"\r\n".to_vec();
            }
            return vec![send(Side::App, data)];
        }
        self.buffer.extend_from_slice(data);
        let mut effects = Vec::new();
        while let Some(line) = take_line(&mut self.buffer) {
            let line = text(&line);
            let Some((code, last)) = code_of(&line) else {
                return fail(RelayFault::Protocol);
            };
            self.reply
                .extend_from_slice(format!("{line}\r\n").as_bytes());
            if !last {
                continue;
            }
            let reply = std::mem::take(&mut self.reply);
            effects.extend(self.on_reply(code, reply));
            if closes(&effects) || effects.contains(&Effect::StartTls) {
                return effects;
            }
            if !matches!(
                self.phase,
                SmtpPhase::Greeting
                    | SmtpPhase::Ehlo
                    | SmtpPhase::StartTls
                    | SmtpPhase::EhloAgain
                    | SmtpPhase::Authenticating(_)
            ) {
                // Relaying began: what the server sent after its last reply is the app's.
                let rest = std::mem::take(&mut self.buffer);
                if !rest.is_empty() {
                    effects.push(send(Side::App, &rest));
                }
                return effects;
            }
        }
        match overlong(&self.buffer) || overlong(&self.reply) {
            true => fail(RelayFault::Protocol),
            false => effects,
        }
    }

    fn on_app_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        match self.phase.clone() {
            SmtpPhase::Greeting
            | SmtpPhase::Ehlo
            | SmtpPhase::StartTls
            | SmtpPhase::EhloAgain
            | SmtpPhase::Authenticating(_) => {
                self.early.extend_from_slice(data);
                match self.early.len() > MAX_EARLY {
                    true => fail(RelayFault::Protocol),
                    false => Vec::new(),
                }
            }
            SmtpPhase::Data => self.body(data),
            SmtpPhase::Chunk { remaining } => self.chunk(remaining, data),
            SmtpPhase::AppEhlo(_) | SmtpPhase::Relaying => {
                self.early.extend_from_slice(data);
                self.commands()
            }
        }
    }

    /// Passes a body on, up to and including its end; what follows is commands again.
    fn body(&mut self, data: &[u8]) -> Vec<Effect> {
        let mut seen = self.tail.clone();
        seen.extend_from_slice(data);
        match find(&seen, END_OF_DATA) {
            Some(at) => {
                let end = at + END_OF_DATA.len() - self.tail.len();
                self.phase = SmtpPhase::Relaying;
                self.tail.clear();
                let mut effects = vec![send(Side::Server, &data[..end])];
                self.early.extend_from_slice(&data[end..]);
                effects.extend(self.commands());
                effects
            }
            None => {
                let keep = seen.len().saturating_sub(END_OF_DATA.len() - 1);
                self.tail = seen[keep..].to_vec();
                vec![send(Side::Server, data)]
            }
        }
    }

    fn chunk(&mut self, remaining: u64, data: &[u8]) -> Vec<Effect> {
        let take = usize::try_from(remaining).map_or(data.len(), |r| r.min(data.len()));
        let mut effects = vec![send(Side::Server, &data[..take])];
        match remaining - take as u64 {
            0 => {
                self.phase = SmtpPhase::Relaying;
                self.early.extend_from_slice(&data[take..]);
                effects.extend(self.commands());
            }
            left => self.phase = SmtpPhase::Chunk { remaining: left },
        }
        effects
    }

    /// Reads the app's complete command lines from `early`.
    fn commands(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        while matches!(self.phase, SmtpPhase::AppEhlo(_) | SmtpPhase::Relaying) {
            let Some(line) = take_line(&mut self.early) else {
                break;
            };
            effects.extend(self.command(&text(&line), &line));
            if closes(&effects) {
                return effects;
            }
        }
        if matches!(self.phase, SmtpPhase::Data | SmtpPhase::Chunk { .. }) {
            // A body or chunk the app sent in the same read as its command.
            let rest = std::mem::take(&mut self.early);
            if !rest.is_empty() {
                effects.extend(self.on_app_bytes(&rest));
            }
        }
        match overlong(&self.early) {
            true => fail(RelayFault::Protocol),
            false => effects,
        }
    }

    fn command(&mut self, line: &str, raw: &[u8]) -> Vec<Effect> {
        let verb = line
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let ehlo = self.ehlo.clone();
        if let SmtpPhase::AppEhlo(reply) = self.phase.clone() {
            return match verb.as_str() {
                "EHLO" => {
                    self.phase = SmtpPhase::Relaying;
                    vec![send(Side::App, &reply.offered_to_app())]
                }
                "HELO" => {
                    self.phase = SmtpPhase::Relaying;
                    vec![send_line(Side::App, &format!("250 {}", reply.domain))]
                }
                "QUIT" => {
                    let mut effects = vec![send_line(Side::App, "221 2.0.0 bye")];
                    effects.extend(finished());
                    effects
                }
                _ => vec![send_line(Side::App, "503 5.5.1 EHLO first")],
            };
        }
        match (verb.as_str(), ehlo) {
            ("STARTTLS", _) => vec![send_line(Side::App, "503 5.5.1 TLS is already active")],
            ("AUTH", _) => vec![send_line(Side::App, "503 5.5.1 already authenticated")],
            ("EHLO", Some(reply)) => vec![send(Side::App, &reply.offered_to_app())],
            ("HELO", Some(reply)) => {
                vec![send_line(Side::App, &format!("250 {}", reply.domain))]
            }
            ("BDAT", _) => {
                if let Some(size) = line
                    .split(' ')
                    .nth(1)
                    .and_then(|n| n.parse::<u64>().ok())
                    .filter(|n| *n > 0)
                {
                    self.phase = SmtpPhase::Chunk { remaining: size };
                }
                vec![send_with_crlf(raw)]
            }
            _ => vec![send_with_crlf(raw)],
        }
    }
}

fn send_with_crlf(line: &[u8]) -> Effect {
    let mut data = line.to_vec();
    data.extend_from_slice(b"\r\n");
    Effect::Send {
        to: Side::Server,
        data,
    }
}

impl Relaying for SmtpRelay {
    fn step(mut self, input: Input) -> (Self, Vec<Effect>) {
        let effects = match input {
            Input::Start => Vec::new(),
            Input::TlsReady if self.phase == SmtpPhase::StartTls => {
                // From here the connection is TLS, as if the endpoint had said so.
                self.plan.endpoint.tls = Tls::Implicit;
                self.phase = SmtpPhase::EhloAgain;
                vec![send_line(Side::Server, &format!("EHLO {EHLO_NAME}"))]
            }
            Input::TlsReady => fail(RelayFault::Protocol),
            Input::Closed(Side::App) => finished(),
            Input::Closed(Side::Server) => match self.phase {
                SmtpPhase::AppEhlo(_)
                | SmtpPhase::Relaying
                | SmtpPhase::Data
                | SmtpPhase::Chunk { .. } => finished(),
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
