//! inferd's parts, as a library so the daemon's modules are tested without a bus: the adapters,
//! the bridge to stoker's provider types, the session machine, the fd session server (`serve`)
//! and the real seams it is hosted over: the router (`router`, `engines`), the engine host
//! (`engines`, `supervise`, `hosts`, `local`, `catalog`), the model turns (`runner`, `cua_run` over `cua_step`)
//! and the audit trail (`audit`); `service` is the `Inference1` object on the bus and `peers` who
//! is calling. Stubs behind frozen interfaces (`speech`, `adapters`) are listed in
//! `FINDINGS.md`.

mod adapters;
pub mod audit;
pub mod auto;
pub mod bridge;
pub mod catalog;
pub mod clock;
pub mod config;
pub mod cua_run;
pub mod cua_step;
pub mod engines;
pub mod hosts;
pub mod local;
pub mod peers;
pub mod replay;
pub mod router;
pub mod runner;
pub mod serve;
pub mod service;
pub mod session;
pub mod speech;
pub mod structured;
pub mod supervise;
pub mod swap;
pub mod tee;

pub use adapters::AdapterModel;

/// The catalog entries the hosted tests use, shared with the unit tests.
#[cfg(test)]
#[path = "../tests/hosting/entries.rs"]
mod entries;

#[cfg(test)]
mod testkit;
