//! Why inferd refuses, and why a model call fails.

use crate::control::StopReason;
use porter_core::DataClass;
use serde::{Deserialize, Serialize};

/// Why a request was not run; each tells the app what to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InferRefusal {
    /// Only cloud models fit, and this class may not leave the machine: the app says so and
    /// may offer Settings.
    #[error("{0:?} data may not leave this computer")]
    RequiresCloud(DataClass),
    /// No model fits and is reachable (local-only with no local runtime running).
    #[error("no model available")]
    Unavailable,
    /// The app holds no grant for a fitting account; it must ask.
    #[error("needs consent")]
    NeedsGrant,
    /// The user refused this app.
    #[error("denied")]
    Denied,
    /// Every fitting account has reached its spend cap for this app.
    #[error("spend cap reached")]
    OverBudget,
    /// The request does not fit the session's need (a chat request on a speech session, a
    /// computer-use need with a class other than `Screen`, `CuaStep` before `CuaBegin`, audio
    /// outside a `Transcribe` turn).
    #[error("request does not fit this session")]
    Unsupported,
}

/// Why one model call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelError {
    /// The runtime or provider could not be reached.
    #[error("unreachable")]
    Unreachable,
    /// Rate limited; retry after this many seconds.
    #[error("rate limited for {0} s")]
    RateLimited(u32),
    /// The key or token was refused.
    #[error("unauthorized")]
    Unauthorized,
    /// The account has no credit left; retrying cannot help until the person tops it up or picks
    /// another model.
    #[error("the account needs payment")]
    PaymentRequired,
    /// The company refused the sign-in and said why (the reason stays in the logs, never here).
    #[error("the company refused the sign-in")]
    SignInRefused,
    /// The provider refused the content.
    #[error("refused by the provider")]
    Refused,
    /// The answer could not be read (or an audio frame was out of order or too long).
    #[error("unreadable answer")]
    Unreadable,
    /// The engine is not running and could not be started.
    #[error("engine not ready")]
    NotReady,
    /// The prompt does not fit the model's context window.
    #[error("context window exceeded")]
    ContextOverflow,
    /// The model's reply is not in the format its dialect promises.
    #[error("reply could not be parsed")]
    Unparseable,
    /// The model reasoned and said nothing: no text and no function call. It is a failure, not an
    /// empty success. Only the lengths are kept, never the thought (ask I4).
    #[error("the reply was only reasoning ({thought_len} bytes)")]
    OnlyThought {
        /// Why the reply ended.
        stop: StopReason,
        /// How many bytes of reasoning it held.
        thought_len: u32,
    },
}
