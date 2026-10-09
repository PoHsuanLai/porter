//! inferd's parts, as a library so the daemon's modules are tested without a bus: the adapters,
//! the bridge to stoker's provider types, the fd session server (`serve`) and the real seams it
//! is hosted over: the router (`engines`), the engine host (`engines`, `supervise`, `hosts`), the
//! model turns (`runner`, `cua_run` over `cua_step`) and the audit trail (`audit`); `service` is
//! the `Inference1` object on the bus and `peers` who is calling. Stubs behind frozen interfaces
//! (`speech`, `adapters`) are listed in `FINDINGS.md`.
//!
//! The logic of routing and the session machine is not here: it is the library `porter-router`
//! (the router, the plan, the catalogue, the local model book, the attached engines as values, the
//! session machine and the seams). `router`, `swap`, `catalog`, `local`, `session` and `startup`
//! below are those modules, re-exported at their old paths; inferd keeps the bus objects, the
//! process hosts, the settings wiring and the start-up.
//!
//! The daemon as one call is `daemon`: [`Config`] (typed; [`Config::from_env`] is the one place
//! the environment is read), [`Daemon::build`] and [`Daemon::run`]. The binary only parses its
//! arguments, listens for signals and calls these.

mod account_news;
mod adapters;
pub mod agent;
pub mod attached;
pub mod audit;
pub mod auto;
pub mod bridge;
pub mod clock;
pub mod cloud;
pub mod config;
pub mod cua_run;
pub mod cua_step;
pub mod daemon;
pub mod engines;
pub mod hosts;
pub mod peers;
pub mod pipeline;
pub mod probe;
pub mod probed;
pub mod replay;
pub mod report;
pub mod runner;
pub mod serve;
pub mod service;
pub mod settings;
pub mod shutdown;
pub mod speech;
pub mod structured;
pub mod supervise;
pub mod tailnet;
pub mod tee;
pub mod watch;

pub use adapters::AdapterModel;
pub use daemon::{Config, Daemon, StartError, StopError};

/// The catalogue, moved to `porter_router::catalog`.
pub use porter_router::catalog;
/// The local model book, moved to `porter_router::local`.
pub use porter_router::local;
/// The router, moved to `porter_router::router`.
pub use porter_router::router;
/// The session machine, moved to `porter_router::session`.
pub use porter_router::session;
/// Why an engine did not start, moved to `porter_router::startup`.
pub use porter_router::startup;
/// What loading a model would take, moved to `porter_router::swap`.
pub use porter_router::swap;

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
