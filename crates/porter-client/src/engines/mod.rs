//! Inference with no inferd (feature `engines`): the app hosts the routing itself and speaks to
//! the OpenAI-compatible engines it points at.
//!
//! ```ignore
//! let ollama = Engine::new(
//!     EngineId::parse("ollama")?, AccountId::parse("ollama")?, "llama3.2:3b",
//!     EngineUrl::parse("http://127.0.0.1:11434/v1")?, Locality::OnDevice,
//!     Dialect::LlamaServer, Tokens(4096),
//! )?;
//! let claude = Engine::new(/* ..., */ Locality::Cloud { region: None }, Dialect::OpenRouter, Tokens(8192))?.with_key();
//! let host = EngineHost::new(
//!     vec![ollama, claude],
//!     vec![Route { slot: Slot::Text, tier: Tier::Balanced, engines: vec![ollama_id, claude_id] }],
//!     Policy::proposed(),            // Mail, Photos, ... stay on this computer
//!     MyKeychain,                    // impl KeySource
//! )?;
//! let accounts = Accounts::over(InProcess::new(service, app).with_broker(host));
//! ```
//!
//! - [`Engine`]: one server, where it is ([`EngineUrl`]), what it serves, which [`Dialect`] of the
//!   chat-completions wire it speaks, where it runs (`Locality`, which the policy's floors read)
//!   and whether a key goes with every request ([`KeyUse`]).
//! - [`Route`]: a row of the table, `(Slot, Tier) -> engines in order of preference`. Nothing is
//!   chosen by default: a need or tier with no row is `Unavailable`. Only language models
//!   (`Need::Llm`, `Slot::Text`) are served; any other need is refused `Unsupported`.
//! - [`Policy`](porter_infer::Policy): the hard rules, applied by `porter_infer::admit` (the one
//!   place they live): `local_only` on drops every cloud engine, and a class whose floor is this
//!   computer never reaches a cloud one, which is `InferRefusal::RequiresCloud(class)` (the
//!   refusal inferd answers).
//! - [`KeySource`]: the app's access to its keys. porter-client stores none.
//!
//! The wire mapping is `porter-bridge` (what inferd uses); retry is inferd's too (three
//! attempts, a quarter of a second doubling to four). Spend caps, the model picker and the audit
//! trail are inferd's and are not here.

mod engine;
mod host;
mod keys;
mod routes;
mod session;
mod turn;
mod wire;

pub use engine::{Dialect, Engine, EngineError, EngineId, EngineUrl, KeyUse};
pub use host::EngineHost;
pub use keys::{KeySource, NoKeys};
pub use routes::Route;
pub use session::EngineSession;

#[cfg(test)]
mod tests;
