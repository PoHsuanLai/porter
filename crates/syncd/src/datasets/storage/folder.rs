//! [`FolderDataset`]: a plain directory as a two-way [`Dataset`], the local side of the app
//! folder mirror. An item's local id and path are its path below the root, `/`-separated.
//!
//! The replica is untrusted input: a path that is empty, absolute or climbs (`..`) is refused
//! before any file is touched. Files are written whole under a dot-temporary name and renamed
//! over their place, so a reader (or the scan) never sees half a file; the temporary names
//! (`.tmp-*`, see `cas`) are never listed. Symbolic links are not followed or listed. Empty
//! folders are not items.

use crate::dataset::{Dataset, DatasetError, DatasetId, fingerprint};
use crate::datasets::photos::cas::{self, hash_file};
use porter_core::Bytes;
use porter_sync::{Blob, ConflictRule, ContentHash, ItemPath, LocalId, Scanned};
use std::io;
use std::path::{Component, Path, PathBuf};

/// The dataset's name.
pub const SLUG: &str = "storage_app_folder";

fn fail(what: &str, why: impl std::fmt::Display) -> DatasetError {
    DatasetError(format!("{what}: {why}"))
}

/// Runs blocking file work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, DatasetError> + Send + 'static,
) -> Result<T, DatasetError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| fail("file work stopped", e))?
}

/// The path below the root that `rel` names, if it names one: plain names only, each segment
/// spelled once (no `.`, no empty segment from `//` or a trailing `/`, no `..`), so one file has
/// one path.
fn below(root: &Path, rel: &str) -> Option<PathBuf> {
    let plain = !rel.is_empty()
        && !rel.contains('\\')
        && rel
            .split('/')
            .all(|name| !matches!(name, "" | "." | "..") && !is_temporary(name))
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    plain.then(|| root.join(rel))
}

fn is_temporary(name: &str) -> bool {
    name.starts_with(".tmp-")
}

fn walk(root: &Path, dir: &Path, into: &mut Vec<Scanned>) -> io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.filter_map(Result::ok) {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if is_temporary(&name) || kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(root, &entry.path(), into)?;
        } else if kind.is_file() {
            let Ok(rel) = entry.path().strip_prefix(root).map(Path::to_path_buf) else {
                continue;
            };
            let Some(rel) = rel.to_str().map(str::to_owned) else {
                continue;
            };
            let (id, size) = match hash_file(&entry.path()) {
                Ok(read) => read,
                // Gone or unreadable between the listing and the read: not this scan's.
                Err(_) => continue,
            };
            into.push(Scanned {
                local: LocalId(rel.clone()),
                path: ItemPath(rel),
                size: Bytes(size),
                hash: ContentHash(id.as_str().to_owned()),
            });
        }
    }
    Ok(())
}

/// Removes the empty directories from `from` up to (not including) `root`.
fn prune(root: &Path, from: &Path) {
    let mut at = from.to_path_buf();
    while at != root && at.starts_with(root) {
        if std::fs::remove_dir(&at).is_err() {
            break;
        }
        match at.parent() {
            Some(parent) => at = parent.to_path_buf(),
            None => break,
        }
    }
}

/// The files under one directory.
#[derive(Debug, Clone)]
pub struct FolderDataset {
    root: PathBuf,
}

impl FolderDataset {
    /// The files under `root` (made when it is first written to).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file(&self, rel: &str) -> Result<PathBuf, DatasetError> {
        below(&self.root, rel).ok_or_else(|| fail("not a path inside the folder", rel))
    }
}

impl Dataset for FolderDataset {
    fn id(&self) -> DatasetId {
        DatasetId::parse(SLUG).expect("a slug")
    }

    /// Both sides changed is shown to the owning app, which settles it with `Sync1.Resolve`.
    fn conflict_rule(&self) -> ConflictRule {
        ConflictRule::ShowInApp
    }

    async fn scan(&self) -> Result<Vec<Scanned>, DatasetError> {
        let root = self.root.clone();
        blocking(move || {
            let mut found = Vec::new();
            walk(&root, &root, &mut found).map_err(|e| fail("cannot list the folder", e))?;
            found.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(found)
        })
        .await
    }

    async fn read(&self, item: &LocalId) -> Result<Blob, DatasetError> {
        let file = self.file(&item.0)?;
        blocking(move || {
            std::fs::read(&file)
                .map(Blob)
                .map_err(|e| fail("cannot read the file", e))
        })
        .await
    }

    async fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        let target = self.file(&path.0)?;
        let moved_from = match at {
            Some(old) if old.0 != path.0 => Some(self.file(&old.0)?),
            _ => None,
        };
        let root = self.root.clone();
        let rel = path.0.clone();
        blocking(move || {
            let (hash, size) = (fingerprint(&content.0), content.0.len() as u64);
            cas::write_atomic(&target, &content.0).map_err(|e| fail("cannot store the file", e))?;
            if let Some(old) = moved_from {
                let _ = std::fs::remove_file(&old);
                if let Some(parent) = old.parent() {
                    prune(&root, parent);
                }
            }
            Ok(Scanned {
                local: LocalId(rel.clone()),
                path: ItemPath(rel),
                size: Bytes(size),
                hash,
            })
        })
        .await
    }

    async fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        let file = self.file(&item.0)?;
        let root = self.root.clone();
        blocking(move || {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(fail("cannot discard the file", e)),
            }
            if let Some(parent) = file.parent() {
                prune(&root, parent);
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    fn dataset(name: &str) -> (FolderDataset, PathBuf) {
        let dir = scratch(name);
        (FolderDataset::new(dir.join("root")), dir)
    }

    #[test]
    fn only_plain_relative_paths_name_a_file_inside_the_folder() {
        let root = Path::new("/r");
        const CASES: &[(&str, bool)] = &[
            ("a.txt", true),
            ("2026/09/a b.txt", true),
            (".hidden", true),
            ("", false),
            ("/etc/passwd", false),
            ("../x", false),
            ("a/../../x", false),
            ("a/./b", false),
            ("a//b", false),
            ("a/b/", false),
            ("a\\b", false),
            (".tmp-1-1", false),
            ("dir/.tmp-1-1", false),
        ];
        for (rel, ok) in CASES {
            assert_eq!(below(root, rel).is_some(), *ok, "{rel:?}");
        }
    }

    #[tokio::test]
    async fn store_scan_read_and_discard_round_trip_with_the_engines_own_fingerprint() {
        let (folder, dir) = dataset("folder-roundtrip");
        let stored = folder
            .store(None, &ItemPath("a/b/c.txt".into()), Blob(b"hello".to_vec()))
            .await
            .expect("store");
        assert_eq!(stored.hash, fingerprint(b"hello"));
        let scan = folder.scan().await.expect("scan");
        assert_eq!(scan, vec![stored.clone()]);
        assert_eq!(
            folder.read(&stored.local).await.expect("read"),
            Blob(b"hello".to_vec())
        );
        // Over the same item, and moved to a new path: the old file and its empty folders go.
        folder
            .store(
                Some(&stored.local),
                &ItemPath("d.txt".into()),
                Blob(b"again".to_vec()),
            )
            .await
            .expect("move");
        assert!(!folder.root().join("a").exists());
        folder
            .discard(&LocalId("d.txt".into()))
            .await
            .expect("discard");
        folder
            .discard(&LocalId("d.txt".into()))
            .await
            .expect("discard again");
        assert_eq!(folder.scan().await.expect("scan"), vec![]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_scan_skips_temporaries_and_links_and_a_bad_path_touches_nothing() {
        let (folder, dir) = dataset("folder-strays");
        std::fs::create_dir_all(folder.root().join("sub")).expect("dir");
        std::fs::write(folder.root().join("keep.txt"), b"k").expect("file");
        std::fs::write(folder.root().join("sub/.tmp-9-9"), b"t").expect("temp");
        std::os::unix::fs::symlink("/etc/hostname", folder.root().join("link")).expect("link");
        let scan = folder.scan().await.expect("scan");
        assert_eq!(
            scan.iter().map(|s| s.path.0.as_str()).collect::<Vec<_>>(),
            ["keep.txt"]
        );
        for bad in ["../escape", "/abs", ""] {
            assert!(
                folder
                    .store(None, &ItemPath(bad.into()), Blob(vec![1]))
                    .await
                    .is_err(),
                "{bad:?}"
            );
            assert!(folder.read(&LocalId(bad.into())).await.is_err(), "{bad:?}");
            assert!(
                folder.discard(&LocalId(bad.into())).await.is_err(),
                "{bad:?}"
            );
        }
        assert!(!dir.join("escape").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_dataset_is_two_way_and_its_conflicts_are_the_apps_to_settle() {
        let (folder, dir) = dataset("folder-rule");
        assert_eq!(folder.id().as_str(), SLUG);
        assert_eq!(folder.conflict_rule(), ConflictRule::ShowInApp);
        assert_eq!(folder.direction(), crate::dataset::Direction::TwoWay);
        let _ = std::fs::remove_dir_all(dir);
    }
}
