//! Running model turns: the part of inferd that takes a routed session and a model of this
//! computer (or any chat provider) and runs the turn, so an app or another daemon can run turns
//! without inferd. No bus and no daemon: a tokio runtime and an OpenAI-compatible engine are all
//! it needs. Routing is its sibling, `porter-router`.
//!
//! The pieces, from the bottom:
//!
//! - [`bridge`]: porter's requests as stoker's turns over a [`porter_router::local::LocalModel`]
//!   (its catalogue entry's sampling and output limit, its engine's flavor); the mapping itself is
//!   `porter-bridge`'s and is re-exported.
//! - [`tee`]: a turn sink that keeps the whole turn for a pure session to read and shows the
//!   person's thinking while it waits.
//! - [`structured`]: a reply of a JSON shape or a choice is validated, and repaired within the
//!   limits ([`structured::Limits`]), before the app is told it.
//! - [`cua_step`] and [`cua_run`]: one computer-use step over a model turn, and the run of steps
//!   of one session.
//! - [`local`]: the turns of one session pinned to a local model ([`local::LocalTurns`]: chat,
//!   embeddings, computer-use steps) over [`local::chat_provider`], retried as the daemon retries;
//!   [`local::EngineUse`] is how the engine's owner is told it is in use.
//! - [`turn`]: [`turn::Turn`], a turn in flight, which implements `porter_router::seams::RunningTurn`.
//! - [`pipeline`]: running a plan stage by stage (hear, then answer) over a `TurnRunner`.
//! - [`audit`]: the audit trail of finished turns (who, through what, how much; never the
//!   content), an `AuditSink` for the session server.
//!
//! Nothing here reads the environment or opens a bus; the daemon passes in what it knows.
//!
//! ```
//! use porter_turns::structured::Limits;
//!
//! // A structured reply is repaired once by default.
//! assert_eq!(Limits::default().repairs.0, 1);
//! ```

pub mod audit;
pub mod bridge;
pub mod cua_run;
pub mod cua_step;
pub mod local;
pub mod pipeline;
pub mod structured;
pub mod tee;
pub mod turn;

pub use local::{EngineUse, LocalTurns};
pub use structured::Limits;
pub use turn::Turn;
