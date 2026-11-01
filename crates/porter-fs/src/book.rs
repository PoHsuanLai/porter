//! A document kept in memory and in a file: the one way the Spaces and the identity uses save a
//! change. [`commit`] writes the new document on the blocking pool and holds it in memory only
//! once it is on disk, so a change that could not be saved is not kept. It tells nobody: the
//! [`Committed`] it returns is for the caller to pass on, after the document is swapped in.

use crate::atomic::AtomicFile;
use serde::Serialize;

/// The document could not be written; the held one is as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveFailed;

impl std::fmt::Display for SaveFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the document could not be written")
    }
}

impl std::error::Error for SaveFailed {}

/// One saved change: the document before it and after it, as the registry's own
/// `Committed { before, after }` says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed<D> {
    /// The document the change started from.
    pub before: D,
    /// The document it left, which is the one now held and saved.
    pub after: D,
}

/// Saves `after` to `file` (none: only in memory) and, once it is on disk, holds it in place of
/// `held`. A document that cannot be written leaves `held` as it was. The caller wakes whoever
/// follows the document after this returns, never from inside the save.
pub async fn commit<D>(
    file: Option<&AtomicFile>,
    held: &mut D,
    after: D,
) -> Result<Committed<D>, SaveFailed>
where
    D: Serialize + Clone,
{
    if let Some(file) = file {
        let text = serde_json::to_string_pretty(&after).map_err(|_| SaveFailed)?;
        let file = file.clone();
        tokio::task::spawn_blocking(move || file.write(&text))
            .await
            .map_err(|_| SaveFailed)?
            .map_err(|_| SaveFailed)?;
    }
    let before = std::mem::replace(held, after.clone());
    Ok(Committed { before, after })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_kit::Scratch;

    #[tokio::test]
    async fn a_saved_document_is_held_and_on_disk_with_the_one_before_as_the_backup() {
        let scratch = Scratch::new("book");
        let file = AtomicFile::new(scratch.path().to_path_buf(), "doc.json");
        let mut held = vec![1];
        commit(Some(&file), &mut held, vec![1, 2])
            .await
            .expect("first");
        let done = commit(Some(&file), &mut held, vec![1, 2, 3])
            .await
            .expect("second");
        assert_eq!((done.before, done.after), (vec![1, 2], vec![1, 2, 3]));
        assert_eq!(held, [1, 2, 3]);
        let text = |p: std::path::PathBuf| std::fs::read_to_string(p).expect("read");
        assert!(text(file.path()).contains('3'));
        assert!(!text(file.backup_path()).contains('3'));
    }

    #[tokio::test]
    async fn a_document_that_cannot_be_written_is_not_held() {
        let scratch = Scratch::new("book");
        // A directory where the file should be: the rename over it fails.
        std::fs::create_dir(scratch.join("doc.json")).expect("dir");
        let file = AtomicFile::new(scratch.path().to_path_buf(), "doc.json");
        let mut held = vec![1];
        assert_eq!(
            commit(Some(&file), &mut held, vec![2]).await,
            Err(SaveFailed)
        );
        assert_eq!(held, [1]);
    }

    #[tokio::test]
    async fn without_a_file_the_document_is_held_in_memory() {
        let mut held = vec![1];
        commit(None, &mut held, vec![2]).await.expect("held");
        assert_eq!(held, [2]);
    }
}
