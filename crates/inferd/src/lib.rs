//! inferd's parts, as a library so the daemon's modules are tested without a bus: the bridge to
//! stoker's provider types, the fd session server (`serve`) and the real seams it is hosted over:
//! the router (`engines`), the engine host (`engines`, `supervise`, `hosts`), the model turns (`runner`, which pins a session's turns to a model and adds the hosted and speech
//! ones) and the audit file's wiring; `service` is the `Inference1` object on the bus and `peers`
//! who is calling. Stubs behind frozen interfaces (`speech`) are listed in
//! `FINDINGS.md`.
//!
//! The logic of routing and the session machine is not here: it is the library `porter-router`
//! (the router, the plan, the catalogue, the local model book, the attached engines as values, the
//! session machine and the seams). Running turns is not here either: it is the library
//! `porter-turns` (a local model's chat, embedding and computer-use turns, the structured-reply
//! check, the audit trail, the stages of a pipeline). inferd keeps the bus objects, the process
//! hosts, the settings wiring and the start-up.
//!
//! The daemon as one call is `daemon`: [`Config`] (typed; [`Config::from_env`] is the one place
//! the environment is read), [`Daemon::build`] and [`Daemon::run`]. The binary only parses its
//! arguments, listens for signals and calls these.

pub mod admit;
pub mod agent;
pub mod attached;
pub mod auto;
pub mod cloud;
pub mod config;
pub mod daemon;
pub mod engines;
pub mod failure;
pub mod hosts;
pub mod installed;
pub mod keys;
pub mod launcher;
pub mod peers;
pub mod pipeline;
pub mod probe;
pub mod probed;
pub mod replay;
pub mod report;
pub mod runner;
pub mod seats;
pub mod serve;
pub mod service;
pub mod settings;
pub mod shutdown;
pub mod speech;
pub mod structured;
pub mod supervise;
pub mod tailnet;
pub mod toolproof;
pub mod watch;

pub use daemon::{Config, Daemon, StartError, StopError};

/// The catalog entries the hosted tests use, shared with the unit tests.
#[cfg(test)]
use porter_router::testkit::entries;

/// The lab engine (an OpenAI-compatible server on a socket) the attached-engine tests share.
#[cfg(test)]
// `say` and `chats` (tests/it/hosting/lab.rs) are used by no test yet; the allow stays for them.
#[allow(dead_code)]
#[path = "../tests/it/hosting/lab.rs"]
mod lab;

/// The fake speech host the speech tests share with the hosted ones.
#[cfg(test)]
#[path = "../tests/it/hosting/speech_host.rs"]
mod speech_host;

#[cfg(test)]
mod testkit;
