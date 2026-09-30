//! The AI broker's pure half (design/31 §5.5): one typed request model whatever the provider,
//! routing that prefers this computer, per-class floors, a local-only switch, spend caps and an
//! audit record without content. inferd runs it; wire adapters implement [`Model`].

mod audit;
mod broker;
mod error;
mod model;
mod policy;
mod reply;
mod request;
mod route;
mod spend;

pub use audit::AuditEntry;
pub use broker::Broker;
pub use error::{InferRefusal, ModelError};
pub use model::{Model, ModelCard};
pub use policy::{ClassFloor, Floor, LocalOnly, Policy};
pub use reply::{ChatReply, EmbedReply, EmbedVector, InferReply, ServedBy, TokenUsage};
pub use request::{
    ChatMessage, ChatRequest, EmbedRequest, ImagePart, InferRequest, MessagePart, ReplyShape, Role,
    Task, TaskRequest,
};
pub use route::{Chosen, RouteAsk, RouteCandidate, TierChoice, route};
pub use spend::{Period, SpendCap, SpendScope, SpendVerdict, cost, spend_verdict};
