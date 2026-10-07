//! Whether an attached engine can answer now: `GET /v1/models` must list the name the catalogue
//! serves the model under. Asked when a session opens and nowhere else (no background polling),
//! and answered with the one thing that is wrong, typed.

use super::key::KeyFileProblem;
use super::target::Target;
use model_http::{
    BodySink, ChunkFlow, Exchange, Framing, HttpClient, HttpError, HttpStatus, ResponseHead,
    RouteRoot, Timeouts, Transport, UrlPath, Verb, WaitMs,
};
use std::path::PathBuf;

/// How long the probe waits at each stage: a tunnel that is up answers at once, and one that
/// hangs is not ready.
const WAIT: WaitMs = WaitMs(3_000);

/// The most of the model list read, in bytes.
const MOST_BODY: usize = 1024 * 1024;

/// The most model names a refusal keeps.
const MOST_NAMES: usize = 64;

/// Why an attached engine cannot answer now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    /// Nothing is at the socket's path: the tunnel is down.
    SocketMissing {
        /// The path.
        path: PathBuf,
    },
    /// The connection was refused or broke: nothing is listening.
    Refused,
    /// The engine wants a bearer token it was not given, or a different one.
    Unauthorized {
        /// 401 or 403.
        status: u16,
    },
    /// The engine answers and does not serve the model.
    ModelAbsent {
        /// The names it does serve (the first of them).
        served: Vec<String>,
    },
    /// The key file was refused, so no request was sent.
    KeyFile(KeyFileProblem),
    /// It answered, but not with a model list, or not in time.
    Unanswered,
}

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotReady::SocketMissing { path } => {
                write!(f, "nothing is at {}: is the tunnel up?", path.display())
            }
            NotReady::Refused => f.write_str("the connection was refused"),
            NotReady::Unauthorized { status } => {
                write!(
                    f,
                    "the engine answered {status}: it wants a bearer token (key_file)"
                )
            }
            NotReady::ModelAbsent { served } => {
                write!(
                    f,
                    "the engine does not serve the model; it serves: {}",
                    served.join(", ")
                )
            }
            NotReady::KeyFile(problem) => write!(f, "{problem}"),
            NotReady::Unanswered => f.write_str("the engine did not answer with a model list"),
        }
    }
}

impl NotReady {
    /// Whether the person set this up wrong (their file), rather than the engine being away.
    pub fn is_setup(&self) -> bool {
        matches!(
            self,
            NotReady::KeyFile(_) | NotReady::Unauthorized { .. } | NotReady::ModelAbsent { .. }
        )
    }
}

/// The status of a response and its body, to a bound.
#[derive(Debug, Default)]
struct Reply {
    status: Option<u16>,
    body: Vec<u8>,
}

impl BodySink for Reply {
    fn head(&mut self, head: &ResponseHead) -> ChunkFlow {
        self.status = Some(head.status.0);
        ChunkFlow::Continue
    }

    fn chunk(&mut self, bytes: &[u8]) -> ChunkFlow {
        let room = MOST_BODY.saturating_sub(self.body.len());
        self.body.extend_from_slice(&bytes[..bytes.len().min(room)]);
        match self.body.len() < MOST_BODY {
            true => ChunkFlow::Continue,
            false => ChunkFlow::Stop,
        }
    }
}

/// The names a `/v1/models` body lists.
fn names_in(body: &[u8]) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let listed = value.get("data")?.as_array()?;
    Some(
        listed
            .iter()
            .filter_map(|model| model.get("id")?.as_str().map(str::to_owned))
            .collect(),
    )
}

/// Asks the engine at `target` whether it serves `served` now.
pub async fn probe(target: &Target, served: &str) -> Result<(), NotReady> {
    if let Some(path) = target.socket()
        && matches!(
            std::fs::symlink_metadata(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        )
    {
        return Err(NotReady::SocketMissing { path: path.clone() });
    }
    let timeouts = Timeouts {
        connect: WAIT,
        first_byte: WAIT,
        idle: WAIT,
    };
    let endpoint = target.endpoint("", timeouts).map_err(NotReady::KeyFile)?;
    let exchange = Exchange {
        verb: Verb::Get,
        root: RouteRoot::Server,
        path: UrlPath("/v1/models".to_owned()),
        body: None,
        framing: Framing::Whole,
    };
    let mut reply = Reply::default();
    let sent = HttpClient::new(endpoint)
        .exchange(&exchange, &mut reply)
        .await;
    match (sent, reply.status) {
        (_, Some(status @ (401 | 403))) => Err(NotReady::Unauthorized { status }),
        (Err(HttpError::Connect | HttpError::Broken), None) => Err(NotReady::Refused),
        (Ok(HttpStatus(200..=299)), _) => match names_in(&reply.body) {
            Some(names) if names.iter().any(|name| name == served) => Ok(()),
            Some(names) => Err(NotReady::ModelAbsent {
                served: names.into_iter().take(MOST_NAMES).collect(),
            }),
            None => Err(NotReady::Unanswered),
        },
        _ => Err(NotReady::Unanswered),
    }
}

#[cfg(test)]
mod tests;
