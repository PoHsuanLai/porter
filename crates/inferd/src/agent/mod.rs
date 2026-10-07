//! inferd as an external coding agent's model endpoint (agent-session ask P4).
//!
//! The agent launcher (`CallerRole::AgentLauncher`, and only it) opens an endpoint over the bus
//! (`org.quire.Inference1.Agents.OpenEndpoint`): for one agent program, over one route (an
//! API-key account, or a model on this computer), for data of one class. inferd binds a
//! per-session listener on `127.0.0.1`, on an ephemeral port, and answers with where it is and a
//! fresh random token the agent sends as its API key. The listener speaks Anthropic Messages and
//! OpenAI Chat Completions (`wire`, `anthropic`, `openai`):
//!
//! * an account route forwards the request, bytes and all, to the account's provider with the
//!   real key, which inferd resolves from accountd for each request (`account`) and which never
//!   leaves the process; the reply is metered against the account's `ai.spend.*` caps;
//! * a model route reads the request into inferd's chat request, runs it on the engine through
//!   the same turn path any app's session uses (`local`), and writes the reply back in the
//!   protocol the agent spoke.
//!
//! Anyone on this computer can reach the port: the token is the whole guard, which is why it is
//! random, compared in constant time, shown to no log, audit line or bus message after the one
//! reply to the launcher, and revoked with the session (`Agents::close`, or the launcher's
//! connection going away).

pub mod account;
pub mod anthropic;
pub mod fail;
pub mod handle;
pub mod http;
pub mod local;
pub mod meter;
pub mod open;
pub mod openai;
pub mod refusal;
pub mod route;
pub mod service;
pub mod session;
pub mod token;
pub mod wire;

pub use open::{Endpoint, OpenRequest};
pub use route::{Models, Route, Target, agent_app};
pub use session::Agents;
pub use token::Token;
