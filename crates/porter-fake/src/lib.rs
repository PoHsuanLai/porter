//! Test-only fakes (a dev crate, CONVENTIONS §3): providers declared by real provider files,
//! three accounts (Storage, Mail, a local Llm), a scripted consent prompter, a fixed clock, an
//! echoing model, and a ready-made service over them. Nothing here touches a network or the
//! user's system.

mod accounts;
mod clock;
mod model;
mod prompter;
mod provider;
mod world;

pub use accounts::{llm_account, mail_account, storage_account};
pub use clock::FixedClock;
pub use model::FakeModel;
pub use prompter::{AskLog, Scripted, ScriptedPrompter};
pub use provider::{FakeProvider, FakeSession, cloud_provider, llm_provider, mail_provider};
pub use world::{FakeService, NOW, fake_service};
