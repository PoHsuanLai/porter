//! A lab machine's engine: an OpenAI-compatible server (vLLM's `/v1/models` and streamed
//! `/v1/chat/completions`, a bearer key when it is given one) on a scratch Unix socket or an
//! ephemeral loopback port. It is the far end of an SSH tunnel in the attached-engine tests; it
//! binds only what the test names and answers only the scratch it is given.

use porter_fake::FakeAddress;
use porter_fake_servers::http::{Request, Response, serve};
use porter_fake_servers::net::{Bind, Listener};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::task::JoinHandle;

/// One request the lab answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// `GET` or `POST`.
    pub method: String,
    /// The path.
    pub path: String,
    /// The `Authorization` header as received.
    pub authorization: Option<String>,
    /// The status it was answered with.
    pub status: u16,
}

#[derive(Debug, Default)]
struct State {
    models: Mutex<Vec<String>>,
    key: Mutex<Option<String>>,
    says: Mutex<Vec<String>>,
    seen: Mutex<Vec<Seen>>,
    chats: Mutex<Vec<Value>>,
}

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running lab engine; dropping it stops the server.
#[derive(Debug)]
pub struct Lab {
    address: FakeAddress,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl Lab {
    /// Serves `models` (and `key`, when the lab wants a bearer) at `bind`; a socket is named
    /// `<dir>/<name>.sock`.
    pub async fn start(bind: &Bind, name: &str, models: &[&str], key: Option<&str>) -> Self {
        let listener = Listener::bind(bind, name).await.expect("bind the lab");
        let address = listener.address().clone();
        let state = Arc::new(State::default());
        *held(&state.models) = models.iter().map(|m| (*m).to_owned()).collect();
        *held(&state.key) = key.map(str::to_owned);
        *held(&state.says) = vec!["The lab says ".to_owned(), "hello.".to_owned()];
        let shared = Arc::clone(&state);
        let task = tokio::spawn(serve(
            listener,
            None,
            Arc::new(move |request| answer(&shared, &request)),
        ));
        Self {
            address,
            state,
            task,
        }
    }

    /// The socket, for a lab bound on one.
    pub fn socket(&self) -> PathBuf {
        match &self.address {
            FakeAddress::Socket(path) => path.clone(),
            FakeAddress::Loopback(_) => panic!("this lab is on a loopback port"),
        }
    }

    /// The port, for a lab bound on loopback.
    pub fn port(&self) -> u16 {
        match &self.address {
            FakeAddress::Loopback(port) => *port,
            FakeAddress::Socket(_) => panic!("this lab is on a socket"),
        }
    }

    /// The models it serves from now on.
    pub fn serve_models(&self, models: &[&str]) {
        *held(&self.state.models) = models.iter().map(|m| (*m).to_owned()).collect();
    }

    /// The deltas the chat answer is made of, one per piece.
    pub fn say(&self, pieces: &[&str]) {
        *held(&self.state.says) = pieces.iter().map(|p| (*p).to_owned()).collect();
    }

    /// Everything it answered, oldest first.
    pub fn seen(&self) -> Vec<Seen> {
        held(&self.state.seen).clone()
    }

    /// The chat request bodies it received, oldest first.
    pub fn chats(&self) -> Vec<Value> {
        held(&self.state.chats).clone()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.task.abort();
        if let FakeAddress::Socket(path) = &self.address {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn refused() -> Response {
    Response::json(
        401,
        &json!({ "error": { "message": "Unauthorized", "type": "invalid_request_error" } }),
    )
}

fn answer(state: &State, request: &Request) -> Response {
    let wanted = held(&state.key).clone();
    let allowed = wanted
        .as_deref()
        .is_none_or(|key| request.bearer() == Some(key));
    let response = if allowed {
        respond(state, request)
    } else {
        refused()
    };
    held(&state.seen).push(Seen {
        method: request.method.clone(),
        path: request.path().to_owned(),
        authorization: request.header("authorization").map(str::to_owned),
        status: response.status,
    });
    response
}

fn respond(state: &State, request: &Request) -> Response {
    let models = held(&state.models).clone();
    match (request.method.as_str(), request.path()) {
        ("GET", "/v1/models") => {
            let data: Vec<Value> = models
                .iter()
                .map(|id| json!({ "id": id, "object": "model", "owned_by": "vllm" }))
                .collect();
            Response::json(200, &json!({ "object": "list", "data": data }))
        }
        ("POST", "/v1/chat/completions") => {
            let wanted: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            held(&state.chats).push(wanted.clone());
            let model = wanted.get("model").and_then(Value::as_str).unwrap_or("");
            if !models.iter().any(|m| m == model) {
                return Response::json(404, &json!({ "error": { "message": "no such model" } }));
            }
            let frame = |delta: Value, finish: Value| {
                let body = json!({ "id": "c1", "object": "chat.completion.chunk", "model": model,
                    "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] });
                format!("data: {body}\n\n")
            };
            let mut body: String = held(&state.says)
                .iter()
                .map(|text| frame(json!({ "content": text }), Value::Null))
                .collect();
            body.push_str(&frame(json!({}), json!("stop")));
            body.push_str(
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
            );
            Response::new(200).typed("text/event-stream", body)
        }
        _ => Response::new(404),
    }
}
