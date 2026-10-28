//! What accountd keeps for `org.quire.Identities1` (`identities_object`) beyond the registry:
//!
//! - [`UseBook`]: the apps that said they signed in with an identity (`RegisterUse`), kept in
//!   `identity-uses.json` beside `registry.json` with the same atomic save and `.bak`
//!   ([`AtomicFile`]). `registry.json`'s format is not touched. An identity that is removed loses
//!   its entry; an app that is gone keeps its line until the identity is, since accountd cannot
//!   tell an app that was uninstalled from one that is not running.
//! - [`Picks`]: the identity each connection was just offered and picked, which is the only thing
//!   that lets it take that identity's password (`TakePassword`), once. A pick is dropped when the
//!   connection offers again, takes it, or leaves the bus.
//!
//! A `identity-uses.json` that cannot be read is never written over: the list is served empty and
//! every change is refused until the person mends or moves the file.

use porter_core::{AppName, IdentityId};
use porter_fs::atomic::AtomicFile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

/// The file's name inside the state directory.
const FILE: &str = "identity-uses.json";
/// The file's own format version.
const VERSION: u16 = 1;

/// `identity-uses.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Document {
    version: u16,
    /// The apps that use each identity.
    uses: BTreeMap<IdentityId, BTreeSet<AppName>>,
}

impl Document {
    fn empty() -> Self {
        Self {
            version: VERSION,
            uses: BTreeMap::new(),
        }
    }
}

/// A change to the uses was refused: the file could not be written, or was refused at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unsaved;

/// The apps that use each identity, as accountd holds them.
#[derive(Debug)]
pub(crate) struct UseBook {
    doc: Document,
    file: Option<AtomicFile>,
    /// False when the file on disk could not be read at start: it is left as it is.
    writable: bool,
}

impl UseBook {
    /// The uses kept in `dir` (none: only in memory, gone when accountd stops). A message for the
    /// log comes back when the file was refused.
    pub(crate) async fn open(dir: Option<&Path>) -> (Self, Option<String>) {
        let file = dir.map(|dir| AtomicFile::new(dir.to_path_buf(), FILE));
        let read = match &file {
            None => Ok(None),
            Some(file) => {
                let file = file.clone();
                tokio::task::spawn_blocking(move || file.read())
                    .await
                    .unwrap_or_else(|e| Err(std::io::Error::other(e)))
            }
        };
        let mut book = Self {
            doc: Document::empty(),
            file,
            writable: true,
        };
        let parsed = read.map(|bytes| {
            bytes.map(|bytes| serde_json::from_slice::<Document>(&bytes).map_err(|e| e.to_string()))
        });
        let refused = match parsed {
            Ok(None) => return (book, None),
            Ok(Some(Ok(doc))) if doc.version == VERSION => {
                book.doc = doc;
                return (book, None);
            }
            Ok(Some(Ok(doc))) => format!("it is of a newer version ({})", doc.version),
            Ok(Some(Err(why))) => why,
            Err(why) => why.to_string(),
        };
        book.writable = false;
        let path = book
            .file
            .as_ref()
            .map(|f| f.path().display().to_string())
            .unwrap_or_default();
        let said = format!(
            "the identity uses file {path} cannot be read ({refused}). It was left as it is; no \
             use can be recorded until it is mended or moved away."
        );
        (book, Some(said))
    }

    /// The apps that use `identity`, in name order.
    pub(crate) fn apps(&self, identity: &IdentityId) -> Vec<AppName> {
        self.doc
            .uses
            .get(identity)
            .map(|apps| apps.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Records that `app` uses `identity`. An app that is recorded already changes nothing and
    /// writes nothing.
    pub(crate) async fn record(
        &mut self,
        identity: IdentityId,
        app: AppName,
    ) -> Result<(), Unsaved> {
        if !self.writable {
            return Err(Unsaved);
        }
        if self
            .doc
            .uses
            .get(&identity)
            .is_some_and(|apps| apps.contains(&app))
        {
            return Ok(());
        }
        let mut doc = self.doc.clone();
        doc.uses.entry(identity).or_default().insert(app);
        self.commit(doc).await
    }

    /// Forgets the apps of `identity`, which was removed. Nothing to forget changes nothing.
    pub(crate) async fn forget(&mut self, identity: &IdentityId) -> Result<(), Unsaved> {
        if !self.writable {
            return Err(Unsaved);
        }
        if !self.doc.uses.contains_key(identity) {
            return Ok(());
        }
        let mut doc = self.doc.clone();
        doc.uses.remove(identity);
        self.commit(doc).await
    }

    /// Saves `doc` and, once it is on disk, holds it. In memory it is held at once.
    async fn commit(&mut self, doc: Document) -> Result<(), Unsaved> {
        if let Some(file) = self.file.clone() {
            let text = serde_json::to_string_pretty(&doc).map_err(|_| Unsaved)?;
            tokio::task::spawn_blocking(move || file.write(&text))
                .await
                .map_err(|_| Unsaved)?
                .map_err(|_| Unsaved)?;
        }
        self.doc = doc;
        Ok(())
    }
}

/// The identity each connection was just offered and picked, by the connection's unique name.
#[derive(Debug, Default)]
pub(crate) struct Picks(Mutex<BTreeMap<String, IdentityId>>);

impl Picks {
    fn held(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, IdentityId>> {
        // Every critical section is a plain data update.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `connection` picked `identity`; any earlier pick of it is replaced.
    pub(crate) fn set(&self, connection: &str, identity: IdentityId) {
        self.held().insert(connection.to_owned(), identity);
    }

    /// Drops what `connection` picked: it offers again, or has left the bus.
    pub(crate) fn clear(&self, connection: &str) {
        self.held().remove(connection);
    }

    /// Spends the pick of `connection` if it is exactly `identity`; whether it was there. Any
    /// other pick is left as it is.
    pub(crate) fn take(&self, connection: &str, identity: &IdentityId) -> bool {
        let mut held = self.held();
        match held.get(connection) {
            Some(picked) if picked == identity => {
                held.remove(connection);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
