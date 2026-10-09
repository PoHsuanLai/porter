//! inferd's parts, as a library so the daemon's modules are tested without a bus: the adapters,
//! the bridge to stoker's provider types, the session machine, the fd session server (`serve`)
//! and the real seams it is hosted over: the router (`router`, `engines`), the engine host
//! (`engines`, `supervise`, `hosts`, `local`, `catalog`), the model turns (`runner`, `cua_run` over `cua_step`)
//! and the audit trail (`audit`); `service` is the `Inference1` object on the bus and `peers` who
//! is calling. Stubs behind frozen interfaces (`speech`, `adapters`) are listed in
//! `FINDINGS.md`.

mod account_news;
mod adapters;
pub mod agent;
pub mod attached;
pub mod audit;
pub mod auto;
pub mod bridge;
pub mod catalog;
pub mod clock;
pub mod cloud;
pub mod config;
pub mod cua_run;
pub mod cua_step;
pub mod engines;
pub mod hosts;
pub mod local;
pub mod peers;
pub mod pipeline;
pub mod probe;
pub mod probed;
pub mod replay;
pub mod report;
pub mod router;
pub mod runner;
pub mod serve;
pub mod service;
pub mod session;
pub mod settings;
pub mod shutdown;
pub mod speech;
pub mod startup;
pub mod structured;
pub mod supervise;
pub mod swap;
pub mod tailnet;
pub mod tee;
pub mod watch;

pub use adapters::AdapterModel;

/// The catalog entries the hosted tests use, shared with the unit tests.
#[cfg(test)]
#[path = "../tests/it/hosting/entries.rs"]
mod entries;

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
