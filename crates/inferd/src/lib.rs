//! inferd's parts, as a library so the daemon's modules are tested without a bus: the adapters,
//! the bridge to stoker's provider types, the session machine, the engine host, the catalog,
//! the computer-use runner and the speech turn. Skeletons behind frozen interfaces; each
//! `todo!()` is listed in `FINDINGS.md`.

mod adapters;
pub mod bridge;
pub mod catalog;
pub mod cua_run;
pub mod engines;
pub mod serve;
pub mod session;
pub mod speech;

pub use adapters::AdapterModel;
