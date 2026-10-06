//! syncd: the sync engine of design/31 §6, as a library so each part is tested without the
//! daemon.
//!
//! - `journal`: the SQLite journal of one dataset of one account (items, anchor, tombstones,
//!   conflicts), every change one transaction.
pub mod callers_file;
pub mod clock;
pub mod dataset;
pub mod driver;
pub mod engine;
pub mod journal;
pub mod paths;
pub mod removal;
pub mod scheduler;
pub mod service;

#[cfg(test)]
mod testing;
