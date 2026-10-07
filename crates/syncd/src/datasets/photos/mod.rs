//! The Photos dataset (W6f, design/31 §6.2-6.3, PLAN D7): built and contract-tested; the Photos
//! app comes later, so syncd does not start it by default (`run`).
//!
//! Two datasets over one local library:
//!
//! - `photos_originals` ([`PhotoOriginals`]): every original stored under the SHA-256 of its
//!   bytes (`ab/<sha256>`), locally and on the replica, so a duplicate import is one file and
//!   a conflict cannot happen (`ConflictRule::Impossible`).
//! - `photos_metadata` ([`PhotoMetadata`]): favourite, caption, albums, deletion and the
//!   imported name as a manifest stamped by hybrid logical clocks, merged field by field, the
//!   later write winning (`ConflictRule::LastWriterPerField`). Each device writes its own
//!   `<device>.json` and reads the others', so no remote file has two writers.
//!
//! - `cas`: [`ContentId`] and the atomic file writes.
//! - `hlc`: [`Hlc`], [`HlcClock`], [`DeviceId`] and the wall-clock seam.
//! - `manifest`: [`Manifest`], its merge and the [`Edit`]s.
//! - `library`: [`PhotoLibrary`], the API the Photos app will call (`import`, `export`,
//!   `manifest`, `edit`).
//! - `originals`, `metadata`: the two [`crate::dataset::Dataset`]s.
//! - `run`: [`start`], which registers both in the hub behind a [`PhotosSwitch`].
//!
//! The library lives in `$XDG_DATA_HOME/porter/photos/<account>/` ([`crate::paths::Paths::photos_dir`]),
//! which `AccountRemoved` wipes with the journals and the PIM mirrors.
//!
//! Not here yet: purging the originals of deleted photos, the replica folders' creation on a
//! fresh account, streaming transfers of originals larger than one `Http` body (FINDINGS).

pub(crate) mod cas;
mod hlc;
mod library;
mod manifest;
mod metadata;
mod originals;
mod run;
#[cfg(test)]
mod tests;

pub use cas::ContentId;
pub use hlc::{DeviceId, Hlc, HlcClock, ManualMillis, Millis, SystemMillis};
pub use library::{Imported, PhotoLibrary, PhotosError, Stored};
pub use manifest::{Edit, Manifest, Mark, Photo, PhotoMeta, Stamped};
pub use metadata::PhotoMetadata;
pub use originals::PhotoOriginals;
pub use run::{PhotosRun, PhotosSwitch, PhotosWiring, StartError, start};
