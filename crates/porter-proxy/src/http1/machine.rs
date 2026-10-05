//! The HTTP relay's steps: heads are read and rewritten, bodies and responses pass unchanged.

use super::chunked::{ChunkParser, Chunked};
use super::head::{Framing, rewrite};
use super::{HttpPhase, HttpRelay};
use crate::fault::RelayFault;
use crate::lines::{closes, fail, finished, send};
use crate::step::{Effect, Input, Relaying, Side};

/// The longest request head accepted.
const MAX_HEAD: usize = 64 * 1024;

const BLANK: &[u8] = b"\r\n\r\n";

fn refusal(fault: RelayFault) -> Vec<Effect> {
    let status = match fault {
        RelayFault::ForeignOrigin => "403 Forbidden",
        _ => "400 Bad Request",
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let mut effects = vec![send(Side::App, response.as_bytes())];
    effects.extend(fail(fault));
    effects
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

impl HttpRelay {
    /// Takes what the app sent, a head at a time and a body at a time.
    fn on_app_bytes(&mut self, data: &[u8]) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut rest = data.to_vec();
        while !rest.is_empty() {
            let step = match self.phase.clone() {
                HttpPhase::Head => self.head_bytes(&mut rest),
                HttpPhase::Body { remaining } => {
                    let take = usize::try_from(remaining).map_or(rest.len(), |r| r.min(rest.len()));
                    let body: Vec<u8> = rest.drain(..take).collect();
                    self.phase = match remaining - take as u64 {
                        0 => HttpPhase::Head,
                        left => HttpPhase::Body { remaining: left },
                    };
                    vec![send(Side::Server, &body)]
                }
                HttpPhase::Chunked => match self.chunks.feed(&rest) {
                    Ok(used) => {
                        let body: Vec<u8> = rest.drain(..used).collect();
                        if self.chunks.state == Chunked::Done {
                            self.phase = HttpPhase::Head;
                        }
                        vec![send(Side::Server, &body)]
                    }
                    Err(_) => refusal(RelayFault::Protocol),
                },
            };
            effects.extend(step);
            if closes(&effects) {
                break;
            }
        }
        effects
    }

    /// Adds `rest` to the head being read; when the blank line is in, rewrites and sends it and
    /// leaves in `rest` what follows the head.
    fn head_bytes(&mut self, rest: &mut Vec<u8>) -> Vec<Effect> {
        if self.head.is_empty() {
            // A client may send blank lines before a request (RFC 9112 2.2).
            let blank = rest
                .iter()
                .take_while(|b| **b == b'\r' || **b == b'\n')
                .count();
            rest.drain(..blank);
        }
        // The blank line can straddle two reads.
        let before = self.head.len().saturating_sub(BLANK.len() - 1);
        self.head.append(rest);
        let Some(at) = find(&self.head[before..], BLANK).map(|at| at + before) else {
            return match self.head.len() > MAX_HEAD {
                true => refusal(RelayFault::Protocol),
                false => Vec::new(),
            };
        };
        let end = at + BLANK.len();
        *rest = self.head.split_off(end);
        let head = std::mem::take(&mut self.head);
        match rewrite(&head, &self.plan) {
            Ok(rewritten) => {
                self.phase = match rewritten.framing {
                    Framing::None => HttpPhase::Head,
                    Framing::Length(remaining) => HttpPhase::Body { remaining },
                    Framing::Chunked => {
                        self.chunks = ChunkParser::default();
                        HttpPhase::Chunked
                    }
                };
                vec![send(Side::Server, &rewritten.head)]
            }
            Err(fault) => refusal(fault),
        }
    }
}

impl Relaying for HttpRelay {
    fn step(mut self, input: Input) -> (Self, Vec<Effect>) {
        let effects = match input {
            Input::Start => Vec::new(),
            // The connection is TLS from the first byte; nothing asks for an upgrade.
            Input::TlsReady => fail(RelayFault::Protocol),
            Input::Closed(_) => finished(),
            Input::Bytes {
                from: Side::Server,
                data,
            } => vec![send(Side::App, &data)],
            Input::Bytes {
                from: Side::App,
                data,
            } => self.on_app_bytes(&data),
        };
        (self, effects)
    }
}
