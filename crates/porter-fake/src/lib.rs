//! Test-only fakes (a dev crate, CONVENTIONS §3): providers declared by real provider files,
//! three accounts (Storage, Mail, a local Llm), scripted sheets, a fixed clock, an in-memory
//! registry store and audit log, an echoing model, the `FakeServer` seam the network fakes
//! implement, and a ready-made service over them. Nothing here touches a network or the user's
//! system.

mod accounts;
mod clock;
mod deadline;
pub mod guard;
mod model;
mod provider;
mod recorders;
mod server;
mod session;
mod sheets;
mod world;

pub use accounts::{llm_account, mail_account, storage_account};
pub use clock::FixedClock;
pub use deadline::{Deadline, GENEROUS};
pub use model::FakeModel;
pub use provider::{
    FakeProvider, FakeSession, FakeSignIn, cloud_provider, llm_provider, mail_provider,
};
pub use recorders::{MemoryStore, RecordingAudit};
pub use server::{FakeAddress, FakeProtocol, FakeServer};
pub use session::{FakeInferSession, Script, ScriptStep};
pub use sheets::{AskLog, Scripted, ScriptedLink, ScriptedSheets};
pub use world::{FakeService, NOW, fake_service};
