//! A dataset held in memory, for the engine's tests (and for tests of the daemon that need one
//! running). Local ids are paths; a test edits files through `put` and `remove`.

use super::{Dataset, DatasetError, DatasetId, fingerprint};
use porter_core::Bytes;
use porter_sync::{Blob, ConflictRule, ItemPath, LocalId, Scanned};
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

/// Files by path.
#[derive(Debug)]
pub struct MemoryDataset {
    id: DatasetId,
    rule: ConflictRule,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryDataset {
    /// An empty dataset named `id` (a slug) settling conflicts by `rule`.
    pub fn new(id: &str, rule: ConflictRule) -> Self {
        Self {
            id: DatasetId::parse(id).expect("a dataset slug"),
            rule,
            files: Mutex::default(),
        }
    }

    fn files(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes a file as the local user would.
    pub fn put(&self, path: &str, bytes: &[u8]) {
        self.files().insert(path.to_owned(), bytes.to_vec());
    }

    /// Deletes a file as the local user would.
    pub fn remove(&self, path: &str) {
        self.files().remove(path);
    }

    /// The file at `path`.
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files().get(path).cloned()
    }

    /// Every file, by path.
    pub fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        self.files().clone()
    }
}

fn scanned(path: &str, bytes: &[u8]) -> Scanned {
    Scanned {
        local: LocalId(path.to_owned()),
        path: ItemPath(path.to_owned()),
        size: Bytes(bytes.len() as u64),
        hash: fingerprint(bytes),
    }
}

impl Dataset for MemoryDataset {
    fn id(&self) -> DatasetId {
        self.id.clone()
    }

    fn conflict_rule(&self) -> ConflictRule {
        self.rule
    }

    async fn scan(&self) -> Result<Vec<Scanned>, DatasetError> {
        Ok(self
            .files()
            .iter()
            .map(|(path, bytes)| scanned(path, bytes))
            .collect())
    }

    async fn read(&self, item: &LocalId) -> Result<Blob, DatasetError> {
        self.get(&item.0)
            .map(Blob)
            .ok_or_else(|| DatasetError(format!("{} is not held", item.0)))
    }

    async fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        let mut files = self.files();
        if let Some(LocalId(old)) = at
            && *old != path.0
        {
            files.remove(old);
        }
        let stored = scanned(&path.0, &content.0);
        files.insert(path.0.clone(), content.0);
        Ok(stored)
    }

    async fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        self.files().remove(&item.0);
        Ok(())
    }
}
