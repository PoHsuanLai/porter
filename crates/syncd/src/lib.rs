//! syncd: the sync engine of design/31 §6, as a library so each part is tested without the
//! daemon.
//!
//! - `journal`: the SQLite journal of one dataset of one account (items, anchor, tombstones,
//!   conflicts), every change one transaction.
//! - `daemon`: the daemon as one call, [`Config`] (typed; [`Config::from_env`] is the one place the
//!   environment is read), [`Daemon::build`] and [`Daemon::run`]. The binary only parses its
//!   arguments and calls these.
pub mod clock;
pub mod daemon;
pub mod dataset;
pub mod datasets;
pub mod driver;
pub mod engine;
pub mod gdrive;
pub mod graph;
pub mod journal;
pub mod paths;
pub mod removal;
pub mod scheduler;
pub mod service;
pub mod webdav;

pub use daemon::{Config, Daemon, StartError};

#[cfg(test)]
mod testing;
