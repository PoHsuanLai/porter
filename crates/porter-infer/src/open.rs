//! What a client may attach when it opens a session, beside the need, the class and the tier.

use crate::ids::Traceparent;
use porter_core::{DataClass, Need, Tier};
use serde::{Deserialize, Serialize};

/// The options of `Inference1.Open`, `Prepare` and `Availability` (the `options` dictionary on the
/// bus, the first frame's options on the socket). Today one key is reserved; an unknown key is
/// ignored by inferd, so a newer client works with an older daemon.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpenOptions {
    /// The caller's trace, so a span started in companiond continues in inferd and one task is
    /// one trace. When absent, inferd starts its own root.
    pub traceparent: Option<Traceparent>,
}

/// What `Inference1.Open` takes, as a frame: the session a connection on the latchkey socket
/// asks for. On the bus these are the method's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenFrame {
    /// The need the session is for.
    pub need: Need,
    /// The data class of everything on it.
    pub class: DataClass,
    /// The tier asked for.
    pub tier: Tier,
    /// The options of the call.
    pub options: OpenOptions,
}

/// The first frame of a connection on the latchkey socket that is a session rather than one
/// accountd call (a bare `AccountsRequest` frame, answered by one `AccountsReply` frame). It is
/// tagged `kind`/`v` like `AccountsRequest`, with a kind that request never has, so the agent
/// reads the first frame once and knows which it is. After it the connection carries
/// `ClientFrame`s one way and `InferEvent`s the other, as the bus's `Open` fd does; a refusal is
/// the first event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum LinkHello {
    /// Open a session.
    Open(OpenFrame),
}
