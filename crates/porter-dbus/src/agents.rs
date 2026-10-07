//! `org.quire.Inference1.Agents` at `/org/quire/Inference1`: inferd as an external coding agent's
//! model endpoint (agent-session ask P4). Only a connection whose caller role is `AgentLauncher`
//! may call it; every other role is refused `AccessDenied`. A call opens a per-session loopback
//! HTTP listener that serves Anthropic Messages and OpenAI Chat Completions, and returns where it
//! is and the secret the agent sends as its "API key". The session ends at `CloseEndpoint`, or
//! when the launcher's bus connection goes away.
//!
//! A separate interface from `org.quire.Inference1` (whose seven members are frozen), on the same
//! bus name and object.

use crate::args::Details;
use zbus::fdo;
use zbus::zvariant::OwnedValue;

/// A route (`(sssas)`): its kind (`account` or `model`), its id (an account id, or a catalogue
/// model id), which model ids it serves (`listed` or `any`) and, for `listed`, the ids.
pub type RouteArg = (String, String, String, Vec<String>);

/// An endpoint (`(sssqsss)`): the session id, the scheme (`http`), the host (`127.0.0.1`), the
/// port, the base URL to give the agent (derived from the three before it and the protocol), the
/// token and the protocol the base URL is for (`anthropic_messages` or `openai_compatible`).
pub type EndpointArg = (String, String, String, u16, String, String, String);

/// The route kind of an API-key account.
pub const ROUTE_ACCOUNT: &str = "account";
/// The route kind of a model on this computer or an attached engine, by catalogue id.
pub const ROUTE_MODEL: &str = "model";
/// A route that serves exactly the ids it lists.
pub const MODELS_LISTED: &str = "listed";
/// A route that serves whatever id the agent names (an account route forwards it as named; a
/// model route answers it with the route's own model).
pub const MODELS_ANY: &str = "any";

/// The prefix of the errors this interface names itself.
pub const AGENT_ERROR_PREFIX: &str = "org.quire.Inference1.Error.";

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Inference1.Agents",
    default_service = "org.quire.Inference1",
    default_path = "/org/quire/Inference1"
)]
pub trait Agents {
    /// Opens an endpoint for the agent `program` (its `AgentProgram` id) over `route`, for data of
    /// `class`, speaking `protocols` (`AgentProtocol` slugs: `anthropic_messages`,
    /// `openai_compatible`; at least one of these two). `options` is reserved (unknown keys are
    /// ignored). The token in the answer is on the bus in this one reply and nowhere else.
    fn open_endpoint(
        &self,
        program: &str,
        route: &RouteArg,
        class: &str,
        protocols: &[&str],
        options: &Details,
    ) -> zbus::Result<EndpointArg>;
    /// Ends a session: its listener is closed and its requests in flight are cancelled.
    fn close_endpoint(&self, session: &str) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct AgentsSkeleton;

#[zbus::interface(name = "org.quire.Inference1.Agents")]
impl AgentsSkeleton {
    fn open_endpoint(
        &self,
        program: String,
        route: RouteArg,
        class: String,
        protocols: Vec<String>,
        options: std::collections::HashMap<String, OwnedValue>,
    ) -> fdo::Result<EndpointArg> {
        let _ = (program, route, class, protocols, options);
        Err(crate::introspect::frozen())
    }

    fn close_endpoint(&self, session: String) -> fdo::Result<()> {
        let _ = session;
        Err(crate::introspect::frozen())
    }
}
