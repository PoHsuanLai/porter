//! Test-only utilities that know nothing of porter (a dev crate, CONVENTIONS §3): a scratch
//! directory that removes itself, the one deadline tests wait with, and the include guard of a
//! crate's `tests/it` binary. Every crate of this workspace may take it as a dev dependency
//! without a cycle, because it depends on nothing of ours. The fakes that do know porter's types
//! (providers, accounts, sheets, the service) are in `porter-fake`.

mod deadline;
pub mod guard;
mod scratch;

pub use deadline::{Deadline, GENEROUS};
pub use scratch::Scratch;
