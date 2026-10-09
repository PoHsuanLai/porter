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
//! The values (`config`, `key`, `model`, `target`, `NotReady`) moved to `porter_router::attached`
//! and stay importable from here; the connection (`check`, `book`, `computers`) is inferd's.
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
#[cfg(test)]
mod config_file_tests;

/// The table and its checks, moved to `porter_router::attached::config`.
pub use porter_router::attached::config;
/// The token's file, moved to `porter_router::attached::key`.
pub use porter_router::attached::key;
/// The catalogue's model of an attached engine, moved to `porter_router::attached::model`.
pub use porter_router::attached::model;
/// How a connect is made, moved to `porter_router::attached::target`.
pub use porter_router::attached::target;

pub use book::AttachedBook;
pub use check::probe;
pub use computers::{
    AddedComputer, AddedFile, AddedModel, AddedProblem, ComputerError, Computers, NewComputer,
    NewModel, NewReach, NewTailnetComputer, relay_socket,
};
pub use porter_router::attached::{
    Attached, AttachedEntry, AttachedError, KeyFile, KeyFileProblem, NotReady, Place, Reach,
    Target, engine_id, local_model, models,
};
