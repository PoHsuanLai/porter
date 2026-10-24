//! Test-only fakes (a dev crate, CONVENTIONS §3): providers declared by real provider files,
//! three accounts (Storage, Mail, a local Llm), scripted sheets, a fixed clock, an in-memory
//! registry store and audit log, the `FakeServer` seam the network fakes implement, and a
//! ready-made service over them. The fake model and session are `porter_infer::testing`. Nothing here touches a network or the user's
//! system.

mod accounts;
mod clock;
mod deadline;
pub mod guard;
mod provider;
mod recorders;
mod scratch;
mod server;
mod sheets;
mod world;

pub use accounts::{llm_account, mail_account, storage_account};
pub use clock::FixedClock;
pub use deadline::{Deadline, GENEROUS};
pub use provider::{
    FakeProvider, FakeSession, FakeSignIn, cloud_provider, llm_provider, mail_provider,
};
pub use recorders::{MemoryStore, RecordingAudit};
pub use scratch::Scratch;
pub use server::{FakeAddress, FakeProtocol, FakeServer};
pub use sheets::{AskLog, Scripted, ScriptedLink, ScriptedSheets};
pub use world::{FakeService, NOW, fake_service};

// moved to porter_infer::testing; re-export kept for one batch
#[cfg(feature = "infer")]
#[doc(hidden)]
pub use porter_infer::testing::{FakeInferSession, FakeModel, Script, ScriptStep};
