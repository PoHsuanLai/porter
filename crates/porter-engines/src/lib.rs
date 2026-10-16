//! The engines that run models on this computer, as data.
//!
//! llama.cpp, vLLM and Strata each come in builds for a processor and a backend (the processor
//! alone, Vulkan, CUDA, ROCm). This crate holds what is known about them and does nothing to the
//! system: it has no network and no runtime, and the only things it does to a disk are reading
//! it and writing an install's record.
//!
//! - [`Manifest`] is the pinned table: for an engine, a processor and a backend, the files to
//!   download with their hashes and sizes, the distro package where one exists, and the
//!   [`seal`] the unpacked tree must have.
//! - [`Host`] is what to choose by: the processor, the cards ([`Gpus`], read from the kernel,
//!   never a name this machine carries) and the distro; the person may force a backend.
//! - [`Roots`] and [`InstallDir`] say where installs live, for everyone or for one person, and
//!   [`Record`] is what an install leaves.
//! - [`Finder`] says which engine is already here and where it came from, without running it.
//! - [`Verifier`] is the check before a person's own install starts: it must still be the pinned
//!   tree.
//!
//! ```
//! use porter_engines::{EngineId, Backend, Manifest, Arch};
//!
//! let manifest = Manifest::pinned();
//! let row = manifest
//!     .row(EngineId::LlamaCpp, Arch::X86_64, Backend::Vulkan)
//!     .expect("a build for Vulkan");
//! assert_eq!(row.version, "b11669");
//! ```

mod failure;
mod found;
mod host;
mod id;
mod layout;
mod manifest;
mod release;
mod rows;
mod seal;
mod verify;

pub use failure::Failure;
pub use found::{Finder, Found, Place, pointed_strata};
pub use host::{Arch, Backend, BackendChoice, GpuVendor, Gpus, preference};
pub use id::EngineId;
pub use layout::{InstallDir, Record, RecordError, Roots, Scope};
pub use manifest::{Artifact, Delivery, Host, Manifest, Offer, PythonEnv, Row};
pub use release::{OsRelease, PackageManager, PackageSource, Releases, Unnumbered, Verified};
pub use rows::packages;
pub use seal::{SealError, TreeSeal, file_digest, hex, seal_of};
pub use verify::{Verdict, Verifier, entry_digests};
