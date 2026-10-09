//! What a computer of the person's is told when it connects to this one and has passed the
//! judgement of `porter_tailnet` (who it is, and that it is theirs): its hello, the list of the
//! models lent, and chat.
//!
//! The protocol is the one inferd already speaks to its own agents' endpoint (OpenAI Chat
//! Completions over HTTP, `agent::http` and `agent::openai`), and the one it speaks to an engine
//! the person attached: so the computer that asks needs nothing new to call this one, only
//! the catalogue id as the model's name. The turn runs through the same path an agent's request
//! to a model on this computer does (`agent::local::run`), with the same audit line (the tokens,
//! never what was said).
//!
//! What may be used is decided here and is not a setting: the models that run on this computer
//! ([`Engines::lendable`]). A request that names any other model (a cloud account's, one on
//! another computer) is answered "not found", and nothing is run or forwarded.
//!
//! A computer that is not yet answered about may ask for its hello (it says it still needs
//! approval) and nothing else; its first request for a model is refused, and raises the
//! question to the person once.

use crate::agent::fail::{Cause, Failure, Shape};
use crate::agent::handle::peek;
use crate::agent::http::{self, ReadError, Request};
use crate::agent::route::{Models, Route, Target};
use crate::agent::session::Ctx;
use crate::agent::token::Token;
use crate::agent::{local, openai};
use crate::audit::AuditOut;
use crate::clock::Clock;
use crate::engines::Engines;
use porter_core::capability::AgentProgram;
use porter_core::{AppId, AppName, DataClass, Isolation, NodeId};
use porter_tailnet::{Approval, Guests, Hello, LentModel, Refusal, State, Visit, Visits};
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// How long a computer that has passed the judgement may take to send its request's head. The
/// house value of the agent endpoint's listener.
const HEAD_WAIT: Duration = Duration::from_secs(120);

/// What serves the computers that pass.
#[derive(Clone)]
pub struct Lender {
    engines: Engines,
    guests: Arc<Guests>,
    audit: Arc<dyn AuditOut>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Lender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lender").finish_non_exhaustive()
    }
}

/// What a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    /// The hello: any computer that passed may ask.
    Hello,
    /// A model: only one the person said yes to.
    Models,
    /// Nothing this serves.
    Nothing,
}

/// The path without the `/v1` an OpenAI base URL carries.
fn bare(request: &Request) -> &str {
    let path = request.path();
    path.strip_prefix("/v1").unwrap_or(path)
}

fn wanted(request: &Request) -> Wanted {
    match (request.method.as_str(), bare(request)) {
        ("GET", "/hello") => Wanted::Hello,
        ("GET", "/models") | ("POST", "/chat/completions") => Wanted::Models,
        _ => Wanted::Nothing,
    }
}

/// The app a computer's requests are counted under in the audit trail.
fn guest_app(node: &NodeId) -> AppId {
    let name = AppName::parse(&format!("org.quire.Guest.{node}"))
        .or_else(|_| AppName::parse("org.quire.Guest"))
        .expect("a fixed app name is one");
    AppId {
        name,
        isolation: Isolation::Unsandboxed,
    }
}

async fn refuse<S: AsyncWrite + Unpin>(stream: &mut S, refusal: &Refusal) {
    let body = json!({
        "error": {"message": refusal.to_string(), "type": "refused", "code": "refused"}
    })
    .to_string();
    let _ = http::write_json(stream, refusal.status(), &[], &body).await;
}

async fn fail<S: AsyncWrite + Unpin>(stream: &mut S, failure: &Failure) {
    let _ = http::write_json(
        stream,
        failure.status(Shape::OpenAi),
        &failure.headers(),
        &failure.body(Shape::OpenAi).to_string(),
    )
    .await;
}

impl Lender {
    /// A lender of the models of `engines`, answering to `guests`.
    pub fn new(
        engines: Engines,
        guests: Arc<Guests>,
        audit: Arc<dyn AuditOut>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            engines,
            guests,
            audit,
            clock,
        }
    }

    async fn serve(&self, visit: Visit) {
        let welcome = visit.welcome().clone();
        // The permit stays until the computer is done.
        let (mut stream, _permit) = visit.into_stream();
        let refused: Mutex<Option<Refusal>> = Mutex::new(None);
        let now = self.clock.now();
        let read = http::read_request(&mut stream, |head| match wanted(head) {
            Wanted::Models => match welcome.admit(&self.guests, now) {
                Ok(()) => true,
                Err(why) => {
                    *refused.lock().unwrap_or_else(PoisonError::into_inner) = Some(why);
                    false
                }
            },
            Wanted::Hello | Wanted::Nothing => true,
        });
        let request = match tokio::time::timeout(HEAD_WAIT, read).await {
            Ok(Ok(request)) => request,
            Err(_) | Ok(Err(ReadError::Closed)) => return,
            Ok(Err(ReadError::Refused)) => {
                let why = refused
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .unwrap_or(Refusal::Off);
                refuse(&mut stream, &why).await;
                return;
            }
            Ok(Err(ReadError::TooLarge)) => {
                let failure = Failure::new(Cause::TooLarge, "the request is too large");
                fail(&mut stream, &failure).await;
                return;
            }
            Ok(Err(ReadError::Malformed)) => {
                let failure = Failure::new(Cause::Invalid, "not an HTTP request");
                fail(&mut stream, &failure).await;
                return;
            }
        };
        if let Err(failure) = self
            .dispatch(&welcome.peer().node, &request, &mut stream)
            .await
        {
            fail(&mut stream, &failure).await;
        }
        let _ = stream.shutdown().await;
    }

    async fn dispatch<S: AsyncWrite + Unpin + Send>(
        &self,
        node: &NodeId,
        request: &Request,
        out: &mut S,
    ) -> Result<(), Failure> {
        match wanted(request) {
            Wanted::Hello => self.hello(node, out).await,
            Wanted::Models if request.method == "GET" => self.models(out).await,
            Wanted::Models => self.chat(node, request, out).await,
            Wanted::Nothing => Err(Failure::new(Cause::NotFound, "nothing is served here")),
        }
    }

    async fn hello<S: AsyncWrite + Unpin>(
        &self,
        node: &NodeId,
        out: &mut S,
    ) -> Result<(), Failure> {
        let models = self
            .engines
            .lendable()
            .await
            .into_iter()
            .map(|(id, name)| LentModel {
                id: id.to_string(),
                name,
            })
            .collect();
        let approval = match self.guests.state_of(node) {
            Some(State::Approved) => Approval::Given,
            Some(State::Denied) | None => Approval::Needed,
        };
        let hello = Hello::new(models, approval);
        http::write_json(out, 200, &[], &hello.to_json())
            .await
            .map_err(|_| Failure::new(Cause::Upstream, "the computer went away"))
    }

    async fn models<S: AsyncWrite + Unpin>(&self, out: &mut S) -> Result<(), Failure> {
        let ids: Vec<String> = self
            .engines
            .lendable()
            .await
            .into_iter()
            .map(|(id, _)| id.to_string())
            .collect();
        let body = openai::models_json(&ids, self.clock.now().0);
        http::write_json(out, 200, &[], &body.to_string())
            .await
            .map_err(|_| Failure::new(Cause::Upstream, "the computer went away"))
    }

    async fn chat<S: AsyncWrite + Unpin + Send>(
        &self,
        node: &NodeId,
        request: &Request,
        out: &mut S,
    ) -> Result<(), Failure> {
        let named = peek(&request.body)?;
        let lent = self.engines.lendable().await;
        let Some((id, _)) = lent.iter().find(|(id, _)| id.as_str() == named.model) else {
            return Err(Failure::new(
                Cause::NotFound,
                "this computer does not lend that model",
            ));
        };
        let id = id.to_string();
        let parsed = openai::parse(&request.body, DataClass::Prompt).map_err(|why| {
            Failure::new(
                Cause::Invalid,
                format!("this computer cannot serve the request: {}", why.0),
            )
        })?;
        let unavailable = || Failure::new(Cause::Overloaded, "the model is not available");
        let ctx = Ctx {
            program: AgentProgram::parse("computer").map_err(|_| unavailable())?,
            app: guest_app(node),
            route: Route {
                target: Target::Model(id.clone()),
                models: Models::Listed(vec![id.clone()]),
            },
            class: DataClass::Prompt,
            // Never compared with anything: the judgement of who asks was made at the door.
            token: Token::fresh().ok_or_else(unavailable)?,
            engines: self.engines.clone(),
            audit: Arc::clone(&self.audit),
            clock: Arc::clone(&self.clock),
        };
        local::run(&ctx, &id, Shape::OpenAi, parsed, out).await
    }
}

impl Visits for Lender {
    fn visit(&self, visit: Visit) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.serve(visit))
    }
}
