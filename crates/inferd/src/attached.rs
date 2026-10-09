//! Engines the person already runs: `[engines.attached."<catalog id>"]` in `inferd.toml` names an
//! engine (vLLM on a lab machine, reached through an SSH tunnel to a local Unix socket, or on a
//! loopback port) that inferd uses and never starts, stops, evicts or kills.
//!
//! ```toml
//! [engines.attached."qwen3-32b"]
//! socket   = "/run/user/1000/lab-vllm.sock"   # or: url = "http://127.0.0.1:8000"
//! key_file = "/home/you/.config/lab/vllm.key" # optional bearer token, mode 0600
//! where    = "my-network"                      # required: "this-device" or "my-network"
//! ```
//!
//! The pieces: [`config`] (the table and the checks the file alone allows), [`key`] (the token's
//! file, read at each connect), [`target`] (how a connect is made), [`check`] (`GET /v1/models`
//! must list the catalogue's served name, and [`NotReady`] says what is wrong when it does not),
//! [`model`] (the [`crate::local::LocalModel`] the catalogue's entry makes) and [`book`] (the
//! engines and what the last look found, looked at when a session opens and at no other time).
//!
//! Routing is the catalogue's: its codec and tool and reasoning parser go through the same bridge
//! as an engine inferd starts, and no accountd grant is asked and no spend is counted. Where the
//! data goes is the person's word (`where`): `this-device` is on this computer, `my-network` is
//! another machine of theirs (`Locality::LocalNetwork`), which an on-device-only floor refuses
//! unless `ai.attached.my_network` is on. There is no TLS: a socket or a loopback address only,
//! and the tunnel's lifetime is the owner's.

pub mod book;
pub mod check;
pub mod computers;
pub mod config;
pub mod key;
pub mod model;
pub mod target;

pub use book::AttachedBook;
pub use check::{NotReady, probe};
pub use computers::{
    AddedComputer, AddedFile, AddedModel, AddedProblem, ComputerError, Computers, NewComputer,
    NewModel, NewReach, NewTailnetComputer, relay_socket,
};
pub use config::{Attached, AttachedEntry, AttachedError, Place, Reach};
pub use key::{KeyFile, KeyFileProblem};
pub use model::{engine_id, local_model};
pub use target::Target;

/// The models of the attached engines `attached` names, over the catalogue's `entries`.
pub fn models(
    attached: &[Attached],
    entries: &[model_catalog::ModelEntry],
    sockets: &std::path::Path,
) -> Result<Vec<crate::local::LocalModel>, AttachedError> {
    attached
        .iter()
        .map(|one| local_model(one, entries, sockets))
        .collect()
}
