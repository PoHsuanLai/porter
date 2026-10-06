//! [`PhotoOriginals`]: the originals directory as a [`Dataset`]. Immutable and content-named, so
//! a conflict cannot happen (`ConflictRule::Impossible`): the same name is the same bytes.
//!
//! `scan` reports each file's name as its SHA-256 without reading it (the name is the hash and
//! the engine never rewrites an original), which keeps a scan of a big library to directory
//! reads; `store` checks the bytes against the name before it writes, so a replica's wrong file
//! is refused rather than kept under a name it does not have.

use super::cas::{self, ContentId};
use super::library::{PhotoLibrary, originals_dir};
use crate::dataset::{Dataset, DatasetError, DatasetId};
use porter_core::Bytes;
use porter_sync::{Blob, ConflictRule, ContentHash, ItemPath, LocalId, Scanned};
use std::io;
use std::path::Path;

/// Every original under `dir`, with its size. Files that are not `ab/<id>` are ignored.
pub(super) fn list(dir: &Path) -> io::Result<Vec<(ContentId, u64)>> {
    let shards = match std::fs::read_dir(dir) {
        Ok(shards) => shards,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut found = Vec::new();
    for shard in shards.filter_map(Result::ok) {
        let Some(shard_name) = shard.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !shard.path().is_dir() {
            continue;
        }
        for file in std::fs::read_dir(shard.path())?.filter_map(Result::ok) {
            let Some(name) = file.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(id) = ContentId::of_rel(&format!("{shard_name}/{name}"))
                && let Ok(meta) = file.metadata()
                && meta.is_file()
            {
                found.push((id, meta.len()));
            }
        }
    }
    found.sort();
    Ok(found)
}

fn fail(what: &str, why: impl std::fmt::Display) -> DatasetError {
    DatasetError(format!("{what}: {why}"))
}

/// Runs blocking file work off the async threads.
pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, DatasetError> + Send + 'static,
) -> Result<T, DatasetError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| fail("file work stopped", e))?
}

/// The originals of one library.
#[derive(Debug, Clone)]
pub struct PhotoOriginals {
    library: PhotoLibrary,
}

impl PhotoOriginals {
    /// The dataset's name.
    pub const SLUG: &'static str = "photos_originals";

    /// The originals of `library`.
    pub fn new(library: PhotoLibrary) -> Self {
        Self { library }
    }

    fn id_of(path: &str) -> Result<ContentId, DatasetError> {
        ContentId::of_rel(path).ok_or_else(|| fail("not an original's path", path))
    }
}

impl Dataset for PhotoOriginals {
    fn id(&self) -> DatasetId {
        DatasetId::parse(Self::SLUG).expect("a slug")
    }

    fn conflict_rule(&self) -> ConflictRule {
        ConflictRule::Impossible
    }

    async fn scan(&self) -> Result<Vec<Scanned>, DatasetError> {
        let dir = originals_dir(self.library.root());
        blocking(move || {
            Ok(list(&dir)
                .map_err(|e| fail("cannot list the originals", e))?
                .into_iter()
                .map(|(id, size)| Scanned {
                    local: LocalId(id.rel()),
                    path: ItemPath(id.rel()),
                    size: Bytes(size),
                    hash: ContentHash(id.as_str().to_owned()),
                })
                .collect())
        })
        .await
    }

    async fn read(&self, item: &LocalId) -> Result<Blob, DatasetError> {
        let file = originals_dir(self.library.root()).join(Self::id_of(&item.0)?.rel());
        blocking(move || {
            std::fs::read(&file)
                .map(Blob)
                .map_err(|e| fail("cannot read the original", e))
        })
        .await
    }

    async fn store(
        &self,
        _at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        let id = Self::id_of(&path.0)?;
        let file = originals_dir(self.library.root()).join(id.rel());
        blocking(move || {
            if ContentId::of(&content.0) != id {
                return Err(fail(
                    "the bytes are not the original they are named for",
                    &id,
                ));
            }
            let size = content.0.len() as u64;
            // The same bytes already here: nothing to write (an import of the same photo).
            if std::fs::metadata(&file).is_ok_and(|m| m.len() == size) {
                return Ok(scanned(&id, size));
            }
            cas::write_atomic(&file, &content.0)
                .map_err(|e| fail("cannot store the original", e))?;
            Ok(scanned(&id, size))
        })
        .await
    }

    async fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        let file = originals_dir(self.library.root()).join(Self::id_of(&item.0)?.rel());
        blocking(move || match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(fail("cannot discard the original", e)),
        })
        .await
    }
}

fn scanned(id: &ContentId, size: u64) -> Scanned {
    Scanned {
        local: LocalId(id.rel()),
        path: ItemPath(id.rel()),
        size: Bytes(size),
        hash: ContentHash(id.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::hlc::{DeviceId, ManualMillis};
    use super::*;
    use crate::dataset::fingerprint;
    use crate::testing::scratch;
    use std::sync::Arc;

    fn dataset(name: &str) -> (PhotoOriginals, PhotoLibrary, std::path::PathBuf) {
        let dir = scratch(name);
        let library = PhotoLibrary::open(
            dir.join("lib"),
            DeviceId::parse("a").expect("device"),
            Arc::new(ManualMillis::at(1)),
        )
        .expect("library");
        (PhotoOriginals::new(library.clone()), library, dir)
    }

    #[tokio::test]
    async fn a_scan_names_each_original_by_its_content_hash_and_ignores_strays() {
        let (originals, library, dir) = dataset("orig-scan");
        let id = ContentId::of(b"pixels");
        originals
            .store(None, &ItemPath(id.rel()), Blob(b"pixels".to_vec()))
            .await
            .expect("store");
        let root = originals_dir(library.root());
        std::fs::write(root.join("stray.txt"), b"x").expect("stray");
        std::fs::create_dir_all(root.join("zz")).expect("dir");
        std::fs::write(root.join("zz/not-an-id"), b"x").expect("stray");
        std::fs::write(root.join(id.rel()).with_file_name(".tmp-1-1"), b"x").expect("temp");
        let scan = originals.scan().await.expect("scan");
        assert_eq!(scan.len(), 1);
        assert_eq!(
            scan[0].hash,
            fingerprint(b"pixels"),
            "the engine's own reading agrees"
        );
        assert_eq!(
            (scan[0].size, scan[0].path.0.as_str()),
            (Bytes(6), id.rel().as_str())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn store_refuses_bytes_that_are_not_the_named_original_and_paths_that_are_not_ids() {
        let (originals, _, dir) = dataset("orig-refuse");
        let id = ContentId::of(b"pixels");
        assert!(
            originals
                .store(None, &ItemPath(id.rel()), Blob(b"other".to_vec()))
                .await
                .is_err()
        );
        for bad in ["../x", "ab/cd", "a.jpg", "/etc/passwd"] {
            assert!(
                originals
                    .store(None, &ItemPath(bad.into()), Blob(vec![]))
                    .await
                    .is_err(),
                "{bad}"
            );
            assert!(originals.read(&LocalId(bad.into())).await.is_err(), "{bad}");
            assert!(
                originals.discard(&LocalId(bad.into())).await.is_err(),
                "{bad}"
            );
        }
        assert_eq!(originals.scan().await.expect("scan"), vec![]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn store_is_idempotent_read_returns_the_bytes_and_discard_of_a_gone_original_is_fine() {
        let (originals, _, dir) = dataset("orig-roundtrip");
        let id = ContentId::of(b"pixels");
        let path = ItemPath(id.rel());
        let first = originals
            .store(None, &path, Blob(b"pixels".to_vec()))
            .await
            .expect("store");
        let second = originals
            .store(None, &path, Blob(b"pixels".to_vec()))
            .await
            .expect("again");
        assert_eq!(first, second);
        assert_eq!(
            originals.read(&first.local).await.expect("read"),
            Blob(b"pixels".to_vec())
        );
        originals.discard(&first.local).await.expect("discard");
        originals.discard(&first.local).await.expect("again");
        assert!(originals.read(&first.local).await.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_dataset_is_immutable_content_with_no_possible_conflict() {
        let (originals, _, dir) = dataset("orig-rule");
        assert_eq!(originals.id().as_str(), "photos_originals");
        assert_eq!(originals.conflict_rule(), ConflictRule::Impossible);
        assert_eq!(
            porter_sync::DatasetKind::PhotosOriginals.conflict_rule(),
            originals.conflict_rule()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
