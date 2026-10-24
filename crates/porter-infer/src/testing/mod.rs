//! Fakes for tests of code built on the broker (feature `testing`): an on-device model that
//! echoes the last user text, and a scripted session that plays canned events per request kind.
//! Nothing here touches a network.

mod model;
mod session;

pub use model::FakeModel;
pub use session::{FakeInferSession, Script, ScriptStep};
