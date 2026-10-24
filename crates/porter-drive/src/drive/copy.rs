//! "Save a Copy": an online document's export, written next to it as a file made on this
//! computer (a local row), "Waiting to upload" until it is sent.

use super::{Drive, NodeId, held};
use crate::index::LocalRow;
use crate::naming;
use crate::source::CopyError;
use crate::words;
use porter_fs::atomic::AtomicWrite;
use porter_sync::ItemKind;
use std::time::{SystemTime, UNIX_EPOCH};

/// The name a file made here is shown by when the index lost it (a crash between its write and
/// the index's), followed by its number.
pub(super) const RECOVERED: &str = "Recovered File";

impl Drive {
    /// Saves the online document `id` as a file in its export format next to it, named as the
    /// document is with the format's extension ("Plan.docx", or "Plan 2.docx" when that is
    /// taken); the copy's node.
    ///
    /// # Errors
    /// The words Files shows: `NOT_A_DOCUMENT` for an item that is no online document,
    /// `NO_FILE_FORMAT` for one with no file format (a Form, a Site, a Map), `TOO_LARGE` past
    /// what the drive exports, `COPY_FAILED` when the drive could not be reached.
    pub async fn save_copy(&self, id: &NodeId) -> Result<NodeId, &'static str> {
        let NodeId::Remote(remote) = id else {
            return Err(words::NOT_A_DOCUMENT);
        };
        let inner = &self.inner;
        let entry = inner
            .entry(remote)
            .ok()
            .flatten()
            .ok_or(words::NOT_A_DOCUMENT)?;
        let ItemKind::Online { export, .. } = &entry.kind else {
            return Err(words::NOT_A_DOCUMENT);
        };
        let (mime, extension) = export
            .as_deref()
            .and_then(|mime| Some((mime, naming::copy_extension(mime)?)))
            .ok_or(words::NO_FILE_FORMAT)?;
        let bytes = inner
            .source
            .export(remote, mime)
            .await
            .map_err(|why| match why {
                CopyError::NoFormat => words::NO_FILE_FORMAT,
                CopyError::TooLarge => words::TOO_LARGE,
                CopyError::Failed => words::COPY_FAILED,
            })?;
        let folder = entry.parent.clone().map_or(NodeId::Top, NodeId::Remote);
        let taken: Vec<String> = inner
            .children(&folder)
            .map_err(|_| words::COPY_FAILED)?
            .into_iter()
            .map(|node| node.name)
            .collect();
        let wanted = format!("{}.{extension}", naming::shown_base(&entry.name));
        let name = naming::free(&wanted, |name| taken.iter().any(|t| t == name));
        let n = {
            let mut index = held(&inner.index);
            let n = index.next_local.max(1);
            index.next_local = n + 1;
            n
        };
        AtomicWrite::PRIVATE
            .write(&inner.local_file(n), &bytes)
            .map_err(|_| words::COPY_FAILED)?;
        let made = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        held(&inner.index).local.push(LocalRow {
            n,
            parent: entry.parent.map(|p| p.0),
            name,
            size: bytes.len() as u64,
            made,
        });
        inner.touched();
        // On the disk before it is answered: a copy said made is never lost.
        if inner.flush_now().is_err() {
            held(&inner.index).local.retain(|row| row.n != n);
            let _ = std::fs::remove_file(inner.local_file(n));
            inner.touched();
            return Err(words::COPY_FAILED);
        }
        let copy = NodeId::Local(n);
        inner.ino(&copy);
        inner.emit(vec![copy.clone(), folder]);
        Ok(copy)
    }
}
