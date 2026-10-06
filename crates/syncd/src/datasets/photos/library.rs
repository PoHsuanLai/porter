//! [`PhotoLibrary`]: the local library of one account, the small API the Photos app will call
//! (`import`, `export`, the manifest reader and the edits), all behind the dataset.
//!
//! The library is `$XDG_DATA_HOME/porter/photos/<account>/`:
//!
//! - `originals/<ab>/<sha256>`: one file per distinct photo, named by its content, so importing
//!   the same bytes again stores nothing.
//! - `manifest/<device>.json`: one manifest per device. A device writes only its own file (the
//!   merge of everything it has seen, plus its edits) and the other devices' files arrive from
//!   the replica as they are, so two devices never write one remote file and the manifest
//!   needs no conflict handling: the view is the field-wise merge of every file.
//! - `device`: this machine's name, kept so it is the same after a restart.

use super::cas::{self, ContentId};
use super::hlc::{DeviceId, HlcClock, Millis};
use super::manifest::{Edit, Manifest};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Why a library call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PhotosError {
    /// A file could not be read or written.
    #[error("{what}: {why}")]
    Io {
        /// What was being done.
        what: String,
        /// Why it failed.
        why: String,
    },
    /// The photo is not in the library.
    #[error("{0} is not in the library")]
    Missing(ContentId),
}

pub(super) fn io_error(what: impl Into<String>, why: &io::Error) -> PhotosError {
    PhotosError::Io {
        what: what.into(),
        why: why.to_string(),
    }
}

/// How an import ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    /// The original was new and is stored now.
    New,
    /// The library already had these bytes: nothing was stored.
    Duplicate,
}

/// One import's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// The photo's identity.
    pub id: ContentId,
    /// Whether it was stored or already there.
    pub stored: Stored,
}

#[derive(Debug)]
struct Inner {
    root: PathBuf,
    device: DeviceId,
    millis: Arc<dyn Millis>,
    clock: Mutex<HlcClock>,
}

/// One account's library. Cheap to clone: the clones are one library, and its edits are
/// serialised.
#[derive(Debug, Clone)]
pub struct PhotoLibrary {
    inner: Arc<Inner>,
}

/// The directory of the originals under a library root.
pub(super) fn originals_dir(root: &Path) -> PathBuf {
    root.join("originals")
}

/// The directory of the manifests under a library root.
pub(super) fn manifest_dir(root: &Path) -> PathBuf {
    root.join("manifest")
}

/// The manifest file a device writes.
pub(super) fn manifest_file(device: &DeviceId) -> String {
    format!("{device}.json")
}

impl PhotoLibrary {
    /// Opens the library at `root` (made when missing). Its device name is the one stored
    /// there, else `device`, which is then stored; the clock starts after every stamp already
    /// in the manifests, so a restart never writes into the past.
    pub fn open(
        root: PathBuf,
        device: DeviceId,
        millis: Arc<dyn Millis>,
    ) -> Result<Self, PhotosError> {
        std::fs::create_dir_all(&root).map_err(|e| io_error("cannot make the library", &e))?;
        let stored = std::fs::read_to_string(root.join("device"))
            .ok()
            .and_then(|text| DeviceId::parse(text.trim()));
        let device = match stored {
            Some(stored) => stored,
            None => {
                cas::write_atomic(&root.join("device"), device.as_str().as_bytes())
                    .map_err(|e| io_error("cannot keep the device name", &e))?;
                device
            }
        };
        let library = Self {
            inner: Arc::new(Inner {
                clock: Mutex::new(HlcClock::new(device.clone())),
                root,
                device,
                millis,
            }),
        };
        {
            let mut clock = library.clock();
            if let Some(latest) = library.manifest()?.latest() {
                clock.observe(latest);
            }
        }
        Ok(library)
    }

    /// This machine's name.
    pub fn device(&self) -> &DeviceId {
        &self.inner.device
    }

    /// The library's directory.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    fn clock(&self) -> MutexGuard<'_, HlcClock> {
        self.inner
            .clock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the original of `id` is, if the library holds it.
    pub fn original(&self, id: &ContentId) -> Option<PathBuf> {
        let path = originals_dir(self.root()).join(id.rel());
        path.is_file().then_some(path)
    }

    /// Every original held.
    pub fn held(&self) -> Result<Vec<ContentId>, PhotosError> {
        Ok(super::originals::list(&originals_dir(self.root()))
            .map_err(|e| io_error("cannot list the originals", &e))?
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }

    /// Imports the file at `path`: stored under its content id unless the library has those
    /// bytes already, and its name recorded in the manifest.
    pub fn import(&self, path: &Path) -> Result<Imported, PhotosError> {
        let mut done = self.import_many(&[path.to_path_buf()])?;
        done.pop().ok_or_else(|| PhotosError::Io {
            what: "import".to_owned(),
            why: "nothing was imported".to_owned(),
        })
    }

    /// Imports several files with one manifest write (a card of photos is one import).
    pub fn import_many(&self, paths: &[PathBuf]) -> Result<Vec<Imported>, PhotosError> {
        let mut imported = Vec::with_capacity(paths.len());
        let mut names = Vec::new();
        for path in paths {
            let what = |step: &str| format!("{step} {}", path.display());
            let (id, _) = cas::hash_file(path).map_err(|e| io_error(what("cannot read"), &e))?;
            let target = originals_dir(self.root()).join(id.rel());
            let stored = if target.is_file() {
                Stored::Duplicate
            } else {
                cas::copy_atomic(path, &target).map_err(|e| io_error(what("cannot store"), &e))?;
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                names.push(Edit::Name(id.clone(), name));
                Stored::New
            };
            imported.push(Imported { id, stored });
        }
        self.edit(names)?;
        Ok(imported)
    }

    /// Copies the original of `id` to `dest`; its size.
    pub fn export(&self, id: &ContentId, dest: &Path) -> Result<u64, PhotosError> {
        let source = self
            .original(id)
            .ok_or_else(|| PhotosError::Missing(id.clone()))?;
        cas::copy_atomic(&source, dest).map_err(|e| io_error("cannot export", &e))?;
        std::fs::metadata(dest)
            .map(|m| m.len())
            .map_err(|e| io_error("cannot export", &e))
    }

    /// The manifest as every device has told it: the merge of every manifest file here.
    pub fn manifest(&self) -> Result<Manifest, PhotosError> {
        read_all(&manifest_dir(self.root())).map_err(|e| io_error("cannot read the manifests", &e))
    }

    /// Writes `edits`, each stamped by this device's clock, into this device's manifest.
    pub fn edit(&self, edits: Vec<Edit>) -> Result<(), PhotosError> {
        if edits.is_empty() {
            return Ok(());
        }
        let mut clock = self.clock();
        let mut manifest = self.manifest()?;
        if let Some(latest) = manifest.latest() {
            clock.observe(latest);
        }
        for edit in edits {
            manifest.apply(edit, clock.tick(self.inner.millis.now_ms()));
        }
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| PhotosError::Io {
            what: "cannot write the manifest".to_owned(),
            why: e.to_string(),
        })?;
        let file = manifest_dir(self.root()).join(manifest_file(self.device()));
        cas::write_atomic(&file, &json).map_err(|e| io_error("cannot write the manifest", &e))
    }
}

/// The merge of every `*.json` in `dir`; a file that is not a manifest is left alone and not
/// read (it stays on disk, so nothing is lost by skipping it).
fn read_all(dir: &Path) -> io::Result<Manifest> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Manifest::default()),
        Err(e) => return Err(e),
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    names.sort();
    Ok(names
        .iter()
        .filter_map(|path| std::fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
        .fold(Manifest::default(), |all, one| all.merged(&one)))
}

#[cfg(test)]
mod tests {
    use super::super::hlc::ManualMillis;
    use super::super::manifest::Mark;
    use super::*;
    use crate::testing::scratch;

    fn open(dir: &Path, device: &str, ms: u64) -> (PhotoLibrary, Arc<ManualMillis>) {
        let millis = Arc::new(ManualMillis::at(ms));
        let library = PhotoLibrary::open(
            dir.join(device),
            DeviceId::parse(device).expect("device"),
            millis.clone(),
        )
        .expect("library");
        (library, millis)
    }

    fn photo(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join("camera").join(name);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(&path, bytes).expect("photo");
        path
    }

    #[test]
    fn an_import_is_stored_by_its_content_and_the_same_bytes_again_store_nothing() {
        let dir = scratch("lib-import");
        let (library, _) = open(&dir, "a", 1_000);
        let first = library
            .import(&photo(&dir, "one.jpg", b"pixels"))
            .expect("import");
        let again = library
            .import(&photo(&dir, "copy of one.jpg", b"pixels"))
            .expect("import");
        let other = library
            .import(&photo(&dir, "two.jpg", b"other"))
            .expect("import");
        assert_eq!(
            (first.stored, again.stored, other.stored),
            (Stored::New, Stored::Duplicate, Stored::New)
        );
        assert_eq!(first.id, again.id);
        assert_eq!(first.id, ContentId::of(b"pixels"));
        let mut held = library.held().expect("held");
        held.sort();
        let mut expected = vec![first.id.clone(), other.id.clone()];
        expected.sort();
        assert_eq!(held, expected);
        let name = library
            .manifest()
            .expect("manifest")
            .photo(&first.id)
            .expect("photo")
            .name;
        assert_eq!(name.as_deref(), Some("one.jpg"), "the first name is kept");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_import_that_cannot_read_its_file_is_an_error_and_stores_nothing() {
        let dir = scratch("lib-import-missing");
        let (library, _) = open(&dir, "a", 1_000);
        assert!(matches!(
            library.import(&dir.join("nope.jpg")),
            Err(PhotosError::Io { .. })
        ));
        assert_eq!(library.held().expect("held"), vec![]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_copies_the_original_out_and_a_missing_photo_is_named() {
        let dir = scratch("lib-export");
        let (library, _) = open(&dir, "a", 1_000);
        let id = library
            .import(&photo(&dir, "one.jpg", b"pixels"))
            .expect("import")
            .id;
        assert_eq!(library.export(&id, &dir.join("out/x.jpg")), Ok(6));
        assert_eq!(
            std::fs::read(dir.join("out/x.jpg")).expect("read"),
            b"pixels"
        );
        let missing = ContentId::of(b"nothing");
        assert_eq!(
            library.export(&missing, &dir.join("out/y.jpg")),
            Err(PhotosError::Missing(missing))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edits_are_stamped_in_order_and_survive_a_restart_even_when_the_clock_went_back() {
        let dir = scratch("lib-restart");
        let (library, millis) = open(&dir, "a", 5_000);
        let id = ContentId::of(b"p");
        library
            .edit(vec![Edit::Favourite(id.clone(), Mark::On)])
            .expect("edit");
        drop(library);
        // The machine's clock is now behind what it wrote before.
        millis.set(10);
        let (library, _) = open(&dir, "a", 10);
        library
            .edit(vec![Edit::Favourite(id.clone(), Mark::Off)])
            .expect("edit");
        assert_eq!(
            library
                .manifest()
                .expect("manifest")
                .photo(&id)
                .expect("photo")
                .favourite,
            Mark::Off,
            "the later edit wins although the wall clock read less"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_device_name_is_the_one_first_stored() {
        let dir = scratch("lib-device");
        let root = dir.join("lib");
        let millis: Arc<dyn Millis> = Arc::new(ManualMillis::at(1));
        let first = PhotoLibrary::open(
            root.clone(),
            DeviceId::parse("laptop").expect("d"),
            millis.clone(),
        )
        .expect("open");
        let second =
            PhotoLibrary::open(root, DeviceId::parse("other").expect("d"), millis).expect("open");
        assert_eq!(
            (first.device().as_str(), second.device().as_str()),
            ("laptop", "laptop")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_view_merges_every_devices_file_and_skips_what_is_not_a_manifest() {
        let dir = scratch("lib-merge");
        let (a, _) = open(&dir, "a", 1_000);
        let id = ContentId::of(b"p");
        a.edit(vec![Edit::Caption(id.clone(), "mine".into())])
            .expect("edit");
        let (b, _) = open(&dir, "b", 2_000);
        b.edit(vec![Edit::Favourite(id.clone(), Mark::On)])
            .expect("edit");
        // b's file arrives in a's library as the replica would put it.
        std::fs::copy(
            manifest_dir(b.root()).join("b.json"),
            manifest_dir(a.root()).join("b.json"),
        )
        .expect("copy");
        std::fs::write(manifest_dir(a.root()).join("junk.json"), b"{not json").expect("junk");
        let view = a.manifest().expect("manifest").photo(&id).expect("photo");
        assert_eq!((view.caption.as_str(), view.favourite), ("mine", Mark::On));
        let _ = std::fs::remove_dir_all(dir);
    }
}
