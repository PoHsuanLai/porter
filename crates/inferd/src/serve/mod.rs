//! The fd session server: one `Open` socket served by the session machine over four seams. The
//! daemon's `Inference1.Open` makes a socketpair, returns one end and spawns [`serve_session`]
//! on the other; the seams are filled by the router, the engine host, the model adapters and the
//! audit log, and by scripted ones in tests.

mod driver;
mod seams;
mod wire;

pub use driver::serve_session;
pub use seams::{
    AuditSink, EngineFailed, EngineHost, Router, RunningTurn, Seams, TurnRunner, TurnStep,
};
pub use wire::MAX_QUEUED_FDS;
