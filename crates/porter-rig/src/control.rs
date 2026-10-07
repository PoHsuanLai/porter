//! The control endpoint: the scenario's levers on the fakes, a small HTTP server on loopback in
//! the rig's own process. `curl` is enough to drive it. Every answer is JSON (`{"ok":true,..}`)
//! except where a route says it returns bytes; an unknown route is `404`, a missing or bad
//! parameter `400`, a lever on a fake that was not started `409`.
//!
//! | route | does |
//! |---|---|
//! | `GET /rig` | the text of `rig.json` |
//! | `POST /issuer/refuse-refreshes?n=N` | the next N refresh requests get `invalid_grant` |
//! | `POST /issuer/revoke[?token=T]` | revokes T at the issuer (default: the planted refresh token) |
//! | `POST /graph/put?path=P` (body) | a remote edit: creates or replaces file P in the app folder |
//! | `POST /graph/delete?path=P` | a remote delete of P |
//! | `GET /graph/file?path=P` | the bytes of P (`404` when there is none) |
//! | `GET /graph/files` | `{"files":[{"path":..,"size":..}]}` |
//! | `POST /graph/expire-delta` | every delta token handed out is stale (`410`) |
//! | `GET /graph/hits` | `{"hits":[{"method","target","status","bearer"}]}` (never the token) |
//! | `GET /imap/attempts`, `/smtp/attempts`, `/pop3/attempts` | `{"attempts":[{"user","outcome"}]}`, oldest first, `outcome` is `accepted` or `refused`; never the password or token |
//! | `POST /ollama/stop`, `POST /ollama/start` | stops the fake Ollama / starts it on the same port |
//! | `GET /ollama/status` | `{"running":bool,"chats":N}` |
//! | `POST /stop` | ends the rig as SIGTERM does |

use crate::planted::OAUTH_REFRESH_TOKEN;
use crate::servers::Ollama;
use porter_fake_servers::http::{Request, Response, Scheme, post_form, send, serve};
use porter_fake_servers::net::{Bind, Listener};
use porter_fake_servers::{GraphHandle, IssuerHandle, MailHandle, browser::split_loopback};
use serde_json::{Value, json};
use std::io;
use std::sync::Arc;
use tokio::sync::Notify;

/// What the levers act on.
#[derive(Debug, Clone)]
pub struct Levers {
    /// The issuer, when it runs.
    pub issuer: Option<IssuerHandle>,
    /// The Graph drive, when it runs.
    pub graph: Option<GraphHandle>,
    /// The IMAP server, when it runs.
    pub imap: Option<MailHandle>,
    /// The SMTP server, when it runs.
    pub smtp: Option<MailHandle>,
    /// The POP3 server, when it runs.
    pub pop3: Option<MailHandle>,
    /// The fake Ollama, when it runs.
    pub ollama: Option<Arc<Ollama>>,
    /// The text `GET /rig` answers.
    pub rig: Arc<std::sync::Mutex<String>>,
    /// Rung by `POST /stop`.
    pub stop: Arc<Notify>,
}

fn ok(extra: Value) -> Response {
    let mut body = json!({ "ok": true });
    if let (Some(into), Some(from)) = (body.as_object_mut(), extra.as_object()) {
        into.extend(from.clone());
    }
    Response::json(200, &body)
}

fn refused(status: u16, why: &str) -> Response {
    Response::json(status, &json!({ "ok": false, "error": why }))
}

fn absent(what: &str) -> Response {
    refused(409, &format!("{what} was not started"))
}

/// Runs an async step from the handler (which is a plain function on a runtime thread).
fn run<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(work))
}

/// The login attempts a mail fake recorded: who tried and how it ended, never what was
/// presented (a password or a bearer token stays in the fake).
fn attempts(server: Option<&MailHandle>, name: &str) -> Response {
    let Some(server) = server else {
        return absent(name);
    };
    let attempts: Vec<Value> = server
        .attempts()
        .iter()
        .map(|a| {
            json!({
                "user": a.user,
                "outcome": if a.accepted { "accepted" } else { "refused" },
            })
        })
        .collect();
    ok(json!({ "attempts": attempts }))
}

impl Levers {
    /// Answers one request.
    pub fn answer(&self, request: &Request) -> Response {
        let query = |name: &str| request.query_value(name);
        match (request.method.as_str(), request.path()) {
            ("GET", "/rig") => {
                let text = self
                    .rig
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                Response::new(200).typed("application/json", text)
            }
            ("POST", "/issuer/refuse-refreshes") => {
                match (
                    self.issuer.as_ref(),
                    query("n").and_then(|n| n.parse::<u32>().ok()),
                ) {
                    (None, _) => absent("the issuer"),
                    (_, None) => refused(400, "n is a number"),
                    (Some(issuer), Some(n)) => {
                        issuer.refuse_refreshes(n);
                        ok(json!({ "n": n }))
                    }
                }
            }
            ("POST", "/issuer/revoke") => {
                let Some(issuer) = self.issuer.as_ref() else {
                    return absent("the issuer");
                };
                let token = query("token").unwrap_or_else(|| OAUTH_REFRESH_TOKEN.to_owned());
                let Ok((address, _)) = split_loopback(issuer.base_url()) else {
                    return refused(500, "the issuer has no loopback address");
                };
                let sent = run(post_form(&address, "/revoke", &[("token", &token)]));
                match sent {
                    Ok(answer) if answer.status == 200 => ok(json!({})),
                    _ => refused(502, "the issuer did not take the revoke"),
                }
            }
            ("POST", "/graph/put") => match (self.graph.as_ref(), query("path")) {
                (None, _) => absent("the Graph drive"),
                (_, None) => refused(400, "path is required"),
                (Some(graph), Some(path)) => {
                    graph.put_file(&path, &request.body);
                    ok(json!({ "path": path, "size": request.body.len() }))
                }
            },
            ("POST", "/graph/delete") => match (self.graph.as_ref(), query("path")) {
                (None, _) => absent("the Graph drive"),
                (_, None) => refused(400, "path is required"),
                (Some(graph), Some(path)) => {
                    graph.delete(&path);
                    ok(json!({ "path": path }))
                }
            },
            ("POST", "/graph/calendars-seed") => match self.graph.as_ref() {
                None => absent("the Graph drive"),
                Some(graph) => {
                    graph.seed_calendars();
                    ok(json!({}))
                }
            },
            ("POST", "/graph/calendar") => match (self.graph.as_ref(), query("id")) {
                (None, _) => absent("the Graph drive"),
                (_, None) => refused(400, "id is required"),
                (Some(graph), Some(id)) => {
                    let name = query("name").unwrap_or_else(|| id.clone());
                    graph.set_calendar(&id, &name, &query("color").unwrap_or_default());
                    ok(json!({ "id": id }))
                }
            },
            ("POST", "/graph/event") => {
                match (self.graph.as_ref(), query("calendar"), query("id")) {
                    (None, ..) => absent("the Graph drive"),
                    (_, None, _) | (_, _, None) => refused(400, "calendar and id are required"),
                    (Some(graph), Some(calendar), Some(id)) => {
                        match serde_json::from_slice::<Value>(&request.body) {
                            Ok(event) => {
                                graph.put_event(&calendar, &id, event);
                                ok(json!({ "calendar": calendar, "id": id }))
                            }
                            Err(_) => refused(400, "the body is not JSON"),
                        }
                    }
                }
            }
            ("POST", "/graph/event-remove") => {
                match (self.graph.as_ref(), query("calendar"), query("id")) {
                    (None, ..) => absent("the Graph drive"),
                    (_, None, _) | (_, _, None) => refused(400, "calendar and id are required"),
                    (Some(graph), Some(calendar), Some(id)) => {
                        graph.remove_event(&calendar, &id);
                        ok(json!({ "calendar": calendar, "id": id }))
                    }
                }
            }
            ("GET", "/graph/file") => match (self.graph.as_ref(), query("path")) {
                (None, _) => absent("the Graph drive"),
                (_, None) => refused(400, "path is required"),
                (Some(graph), Some(path)) => match graph.file(&path) {
                    Some(bytes) => Response::new(200).typed("application/octet-stream", bytes),
                    None => refused(404, "no such file"),
                },
            },
            ("GET", "/graph/files") => match self.graph.as_ref() {
                None => absent("the Graph drive"),
                Some(graph) => {
                    let files: Vec<Value> = graph
                        .files()
                        .into_iter()
                        .map(|(path, size)| json!({ "path": path, "size": size }))
                        .collect();
                    ok(json!({ "files": files }))
                }
            },
            ("POST", "/graph/expire-delta") => match self.graph.as_ref() {
                None => absent("the Graph drive"),
                Some(graph) => {
                    graph.expire_delta_tokens();
                    ok(json!({}))
                }
            },
            ("GET", "/graph/hits") => match self.graph.as_ref() {
                None => absent("the Graph drive"),
                Some(graph) => {
                    let hits: Vec<Value> = graph
                        .hits()
                        .iter()
                        .map(|h| {
                            json!({
                                "method": h.method, "target": h.target, "status": h.status,
                                "bearer": h.authorization.is_some(),
                            })
                        })
                        .collect();
                    ok(json!({ "hits": hits }))
                }
            },
            ("GET", "/imap/attempts") => attempts(self.imap.as_ref(), "the IMAP server"),
            ("GET", "/smtp/attempts") => attempts(self.smtp.as_ref(), "the SMTP server"),
            ("GET", "/pop3/attempts") => attempts(self.pop3.as_ref(), "the POP3 server"),
            ("POST", "/ollama/stop") => match self.ollama.as_ref() {
                None => absent("the fake Ollama"),
                Some(ollama) => ok(json!({ "stopped": run(ollama.stop()) })),
            },
            ("POST", "/ollama/start") => match self.ollama.as_ref() {
                None => absent("the fake Ollama"),
                Some(ollama) => match run(ollama.start()) {
                    Ok(started) => ok(json!({ "started": started, "port": ollama.port() })),
                    Err(why) => refused(500, &format!("cannot bind the old port: {why}")),
                },
            },
            ("GET", "/ollama/status") => match self.ollama.as_ref() {
                None => absent("the fake Ollama"),
                Some(ollama) => ok(json!({
                    "running": ollama.is_running(), "chats": ollama.chats(), "port": ollama.port(),
                })),
            },
            ("POST", "/stop") => {
                self.stop.notify_one();
                ok(json!({}))
            }
            _ => refused(404, "no such lever"),
        }
    }
}

/// Binds the control endpoint on loopback and serves it on a task; its URL.
pub async fn serve_levers(levers: Levers) -> io::Result<(String, tokio::task::JoinHandle<()>)> {
    let listener = Listener::bind(&Bind::Loopback, "control").await?;
    let port = porter_fake_servers::net::port_of(listener.address());
    let task = tokio::spawn(serve(
        listener,
        None,
        Arc::new(move |request: Request| levers.answer(&request)),
    ));
    Ok((format!("http://127.0.0.1:{port}"), task))
}

/// A request to a control endpoint, for the tests and for scenarios written in Rust.
pub async fn call(control: &str, method: &str, target: &str, body: &[u8]) -> io::Result<Response> {
    let (address, _) = split_loopback(control)?;
    send(
        &address,
        Scheme::Http,
        &Request::new(method, target).with_body(body.to_vec()),
    )
    .await
}
