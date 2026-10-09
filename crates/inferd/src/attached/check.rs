//! Whether an attached engine can answer now: `GET /v1/models` must list the name the catalogue
//! serves the model under. Asked when a session opens and nowhere else (no background polling),
//! and answered with the one thing that is wrong, typed.

use super::target::Target;
use model_http::{
    BodySink, ChunkFlow, Exchange, Framing, HttpClient, HttpError, HttpStatus, ResponseHead,
    RouteRoot, Timeouts, Transport, UrlPath, Verb, WaitMs,
};

/// Why an attached engine cannot answer now, moved to `porter_router::attached`.
pub use porter_router::attached::NotReady;

/// How long the probe waits at each stage: a tunnel that is up answers at once on a quiet
/// computer and in seconds on a loaded one (a connection made in 20 ms idle took seconds), and
/// one that hangs is not ready. Only an engine that hangs makes anyone wait this long; a missing
/// socket or a refusal ends the look at once.
const WAIT: WaitMs = WaitMs(30_000);

/// How long the probe waits at each stage for a computer on the Tailscale network: its relay
/// asks Tailscale who is at the address, twice, and the computer's own answer waits for the same
/// on its side, so a computer that is up but busy takes far longer than a tunnel the person
/// holds open. Not a short deadline a loaded computer would trip over.
const RELAYED_WAIT: WaitMs = WaitMs(60_000);

/// The most of the model list read, in bytes.
const MOST_BODY: usize = 1024 * 1024;

/// The most model names a refusal keeps.
const MOST_NAMES: usize = 64;

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
    let wait = if target.is_relayed() {
        RELAYED_WAIT
    } else {
        WAIT
    };
    let timeouts = Timeouts {
        connect: wait,
        first_byte: wait,
        idle: wait,
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
