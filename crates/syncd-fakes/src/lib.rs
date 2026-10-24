//! Fakes of what syncd serves (test tooling: `publish = false`, never installed, never in
//! `dist/`).
//!
//! - `storage`: a fake `org.quire.Storage1`, two drives of real files whose states move, for
//!   building Files before syncd serves the real one. The binary `porter-rig-storage` (its name
//!   and flags kept from when it lived in porter-rig, because the shell session runs it) serves
//!   it on the session bus.

pub mod storage;
