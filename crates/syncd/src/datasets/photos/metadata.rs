//! [`PhotoMetadata`]: the manifest directory as a [`Dataset`], rule `LastWriterPerField`.
//!
//! The directory holds one `<device>.json` per device. Each device uploads its own file and
//! pulls the others', so the engine never sees two writers of one remote file; the field-wise
//! merge by clock is [`super::manifest::Manifest::merged`], applied when the library reads the
//! directory. A file from another device is stored as the replica has it (never parsed here:
//! one device's damaged file must not stop the rest syncing), and the reader skips it.

use super::cas;
use super::hlc::DeviceId;
use super::library::{PhotoLibrary, manifest_dir};
use super::originals::blocking;
use crate::dataset::{Dataset, DatasetError, DatasetId, fingerprint};
use porter_core::Bytes;
use porter_sync::{Blob, ConflictRule, ItemPath, LocalId, Scanned};
use std::io;

fn fail(what: &str, why: impl std::fmt::Display) -> DatasetError {
    DatasetError(format!("{what}: {why}"))
}

/// The device a manifest path `<device>.json` is of.
fn device_of(path: &str) -> Result<DeviceId, DatasetError> {
    path.strip_suffix(".json")
        .and_then(DeviceId::parse)
        .ok_or_else(|| fail("not a manifest's path", path))
}

/// The manifests of one library.
#[derive(Debug, Clone)]
pub struct PhotoMetadata {
    library: PhotoLibrary,
}

impl PhotoMetadata {
    /// The dataset's name.
    pub const SLUG: &'static str = "photos_metadata";

    /// The manifests of `library`.
    pub fn new(library: PhotoLibrary) -> Self {
        Self { library }
    }

    fn file_of(&self, path: &str) -> Result<std::path::PathBuf, DatasetError> {
        device_of(path).map(|_| manifest_dir(self.library.root()).join(path))
    }
}

impl Dataset for PhotoMetadata {
    fn id(&self) -> DatasetId {
        DatasetId::parse(Self::SLUG).expect("a slug")
    }

    fn conflict_rule(&self) -> ConflictRule {
        ConflictRule::LastWriterPerField
    }

    async fn scan(&self) -> Result<Vec<Scanned>, DatasetError> {
        let dir = manifest_dir(self.library.root());
        blocking(move || {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(fail("cannot list the manifests", e)),
            };
            let mut found = Vec::new();
            for entry in entries.filter_map(Result::ok) {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if device_of(&name).is_err() {
                    continue;
                }
                let bytes = std::fs::read(entry.path()).map_err(|e| fail("cannot read", e))?;
                found.push(Scanned {
                    local: LocalId(name.clone()),
                    path: ItemPath(name),
                    size: Bytes(bytes.len() as u64),
                    hash: fingerprint(&bytes),
                });
            }
            found.sort_by(|a, b| a.local.cmp(&b.local));
            Ok(found)
        })
        .await
    }

    async fn read(&self, item: &LocalId) -> Result<Blob, DatasetError> {
        let file = self.file_of(&item.0)?;
        blocking(move || {
            std::fs::read(&file)
                .map(Blob)
                .map_err(|e| fail("cannot read the manifest", e))
        })
        .await
    }

    async fn store(
        &self,
        _at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        let file = self.file_of(&path.0)?;
        let name = path.0.clone();
        blocking(move || {
            cas::write_atomic(&file, &content.0)
                .map_err(|e| fail("cannot store the manifest", e))?;
            Ok(Scanned {
                local: LocalId(name.clone()),
                path: ItemPath(name),
                size: Bytes(content.0.len() as u64),
                hash: fingerprint(&content.0),
            })
        })
        .await
    }

    async fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        let file = self.file_of(&item.0)?;
        blocking(move || match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(fail("cannot discard the manifest", e)),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::super::cas::ContentId;
    use super::super::hlc::ManualMillis;
    use super::super::manifest::{Edit, Mark};
    use super::*;
    use crate::testing::scratch;
    use std::sync::Arc;

    fn dataset(name: &str) -> (PhotoMetadata, PhotoLibrary, std::path::PathBuf) {
        let dir = scratch(name);
        let library = PhotoLibrary::open(
            dir.join("lib"),
            DeviceId::parse("a").expect("device"),
            Arc::new(ManualMillis::at(1_000)),
        )
        .expect("library");
        (PhotoMetadata::new(library.clone()), library, dir)
    }

    #[tokio::test]
    async fn a_scan_lists_each_devices_manifest_and_nothing_else() {
        let (metadata, library, dir) = dataset("meta-scan");
        assert_eq!(metadata.scan().await.expect("scan"), vec![]);
        library
            .edit(vec![Edit::Favourite(ContentId::of(b"p"), Mark::On)])
            .expect("edit");
        std::fs::write(manifest_dir(library.root()).join("notes.txt"), b"x").expect("stray");
        std::fs::write(manifest_dir(library.root()).join("Bad Name.json"), b"x").expect("stray");
        let scan = metadata.scan().await.expect("scan");
        assert_eq!(scan.len(), 1);
        assert_eq!(scan[0].path.0, "a.json");
        let bytes = metadata.read(&scan[0].local).await.expect("read");
        assert_eq!(scan[0].hash, fingerprint(&bytes.0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn another_devices_manifest_is_stored_as_is_and_merged_into_the_view() {
        let (metadata, library, dir) = dataset("meta-store");
        let id = ContentId::of(b"p");
        let theirs = {
            let other = PhotoLibrary::open(
                dir.join("other"),
                DeviceId::parse("b").expect("device"),
                Arc::new(ManualMillis::at(9_000)),
            )
            .expect("library");
            other
                .edit(vec![Edit::Favourite(id.clone(), Mark::On)])
                .expect("edit");
            std::fs::read(manifest_dir(other.root()).join("b.json")).expect("read")
        };
        let stored = metadata
            .store(None, &ItemPath("b.json".into()), Blob(theirs.clone()))
            .await
            .expect("store");
        assert_eq!(stored.hash, fingerprint(&theirs));
        assert_eq!(
            library
                .manifest()
                .expect("manifest")
                .photo(&id)
                .expect("photo")
                .favourite,
            Mark::On
        );
        // Garbage from a device is kept, and the view skips it.
        metadata
            .store(None, &ItemPath("c.json".into()), Blob(b"{broken".to_vec()))
            .await
            .expect("store");
        assert_eq!(
            library
                .manifest()
                .expect("manifest")
                .photo(&id)
                .expect("photo")
                .favourite,
            Mark::On
        );
        metadata
            .discard(&LocalId("c.json".into()))
            .await
            .expect("discard");
        metadata
            .discard(&LocalId("c.json".into()))
            .await
            .expect("again");
        assert_eq!(metadata.scan().await.expect("scan").len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn only_a_device_json_path_is_a_manifest() {
        let (metadata, _, dir) = dataset("meta-paths");
        for bad in ["../a.json", "a.json/x", "a", "A.json", "a b.json", ".json"] {
            assert!(
                metadata
                    .store(None, &ItemPath(bad.into()), Blob(vec![]))
                    .await
                    .is_err(),
                "{bad}"
            );
            assert!(metadata.read(&LocalId(bad.into())).await.is_err(), "{bad}");
        }
        assert_eq!(metadata.id().as_str(), "photos_metadata");
        assert_eq!(metadata.conflict_rule(), ConflictRule::LastWriterPerField);
        assert_eq!(
            porter_sync::DatasetKind::PhotosMetadata.conflict_rule(),
            metadata.conflict_rule()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
