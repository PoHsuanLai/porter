//! syncd: the sync engine of design/31 §6, as a library so each part is tested without the
//! daemon.
//!
//! - `journal`: the SQLite journal of one dataset of one account (items, anchor, tombstones,
//!   conflicts), every change one transaction.
//! - `keep`: syncd's own accounts: its own store and keyring namespace (`porter.syncd`),
//!   "Continue as" sign-in, the sync switches and removal. `settings`: the Settings module
//!   (`org.quire.SettingsModule1`) over them.
//! - `daemon`: the daemon as one call, [`Config`] (typed; [`Config::from_env`] is the one place the
//!   environment is read), [`Daemon::build`] and [`Daemon::run`]. The binary only parses its
//!   arguments and calls these.
// The PIM supervisor's future nests the whole account service's generics; peephole's caller
// types (porter-dbus ProcCallers) take its layout past the default depth of 128.
#![recursion_limit = "256"]
pub mod clock;
pub mod daemon;
pub mod dataset;
pub mod datasets;
pub mod driver;
pub mod engine;
pub mod gdrive;
pub mod graph;
pub mod journal;
pub mod keep;
pub mod paths;
pub mod removal;
pub mod scheduler;
pub mod service;
pub mod settings;
pub mod webdav;
pub mod words;

pub use daemon::{Config, Daemon, StartError};

#[cfg(test)]
mod testing;
