//! Writes to a whole drive, beside the app folder's [`crate::Replica`]: what a drive's folder on
//! this computer sends when a person changes it. Items are named by id and placed by a parent id
//! and a name (never a path: a drive's names are shown through naming rules this crate does not
//! know). Every change of an existing item carries the version it was made against, and a stale
//! one is a [`PutRefused::Conflict`], never an overwrite. Uploads read their file a chunk at a
//! time; a large one runs in an upload session that can be resumed after a restart. A delete
//! sends the item to the drive's recycle bin, never a permanent delete.

use crate::item::{BaseVersion, RemoteId, RemoteVersion};
use crate::refusal::PutRefused;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;

/// Where an item goes: a folder by id (`None`: the top of the drive) and its name there.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    /// The folder.
    pub parent: Option<RemoteId>,
    /// The name, as the drive will have it.
    pub name: String,
}

/// What an upload makes or replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadTo {
    /// A new file.
    New(Place),
    /// New content for a file that exists: the same id, a new version, so the drive keeps the
    /// history.
    Replace(RemoteId),
}

/// A drive's upload session in flight: its address. The address takes bytes with no sign-in,
/// so it is a credential of its own: stored owner-only, and never shown or logged (its `Debug`
/// says nothing of it).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Session(String);

impl Session {
    /// The session at `address`.
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    /// Its address, for the replica that sends to it.
    pub fn address(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Session(..)")
    }
}

/// What an upload sends: a file of `size` bytes, read a chunk at a time, never whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    path: PathBuf,
    size: u64,
}

impl Content {
    /// The first `size` bytes of the file at `path`.
    pub fn new(path: PathBuf, size: u64) -> Self {
        Self { path, size }
    }

    /// How many bytes it is.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// `len` bytes from `offset` (fewer at its end).
    ///
    /// # Errors
    /// The file could not be read, or is shorter than its size.
    pub fn read(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let end = offset.saturating_add(len).min(self.size);
        let len = usize::try_from(end.saturating_sub(offset)).map_err(std::io::Error::other)?;
        let mut buffer = vec![0; len];
        std::fs::File::open(&self.path)?.read_exact_at(&mut buffer, offset)?;
        Ok(buffer)
    }
}

/// What an upload tells as it goes.
pub trait Progress: Send + Sync {
    /// The session it opened (`None`: the one it had is over), to be kept with the change so
    /// the upload resumes after a restart.
    fn session(&self, session: Option<&Session>);

    /// How many bytes the drive has so far.
    fn sent(&self, bytes: u64);

    /// Whether the upload is to stop between chunks (the drive was paused): it ends as
    /// [`PutRefused::Transient`], its session kept, to resume later.
    fn held(&self) -> bool {
        false
    }
}

/// A [`Progress`] that keeps nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unheard;

impl Progress for Unheard {
    fn session(&self, _: Option<&Session>) {}

    fn sent(&self, _: u64) {}
}

/// One upload.
pub struct Upload<'a> {
    /// What it makes or replaces.
    pub to: UploadTo,
    /// What a replaced file was when the change was made (`Absent` for a new file).
    pub base: BaseVersion,
    /// The bytes.
    pub content: &'a Content,
    /// A session from before, resumed when the drive still has it.
    pub resume: Option<Session>,
    /// Told of the session and of the bytes sent.
    pub progress: &'a dyn Progress,
}

impl std::fmt::Debug for Upload<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upload")
            .field("to", &self.to)
            .field("base", &self.base)
            .field("content", &self.content)
            .field("resume", &self.resume)
            .finish_non_exhaustive()
    }
}

/// An item as the drive has it after a write.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Written {
    /// Its id.
    pub id: RemoteId,
    /// Its version now.
    pub version: RemoteVersion,
}

/// The writes of a whole drive. Each is one step: a folder, a file's bytes, a new place, the
/// recycle bin.
pub trait DriveWrite: Send + Sync {
    /// Whether the drive takes `name` for a file or folder. A name it refuses is refused when it
    /// is made here, not when it is sent.
    fn accepts_name(&self, name: &str) -> bool;

    /// Makes a folder at `place`. A name taken there is a `Conflict` (`Exists`).
    fn make_folder(
        &self,
        place: &Place,
    ) -> impl Future<Output = Result<Written, PutRefused>> + Send;

    /// Sends a file's bytes: a small one in one request, a larger one in an upload session, in
    /// chunks.
    fn upload(
        &self,
        upload: Upload<'_>,
    ) -> impl Future<Output = Result<Written, PutRefused>> + Send;

    /// Renames and/or moves `item` to `to`, if the drive still has `base`.
    fn relocate(
        &self,
        item: &RemoteId,
        base: &BaseVersion,
        to: &Place,
    ) -> impl Future<Output = Result<Written, PutRefused>> + Send;

    /// Sends `item` to the drive's recycle bin, if the drive still has `base`; a folder takes
    /// what it holds.
    fn trash(
        &self,
        item: &RemoteId,
        base: &BaseVersion,
    ) -> impl Future<Output = Result<(), PutRefused>> + Send;

    /// Gives up an upload session that will not be resumed (the bytes it was sending changed),
    /// so the drive drops what it holds of it. A drive that drops sessions only in time does
    /// nothing.
    fn abandon(&self, session: &Session) -> impl Future<Output = ()> + Send {
        let _ = session;
        async {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_never_shows_its_address() {
        let session = Session::new("https://up.example.test/s/secret");
        assert_eq!(format!("{session:?}"), "Session(..)");
        assert_eq!(session.address(), "https://up.example.test/s/secret");
        let json = serde_json::to_string(&session).expect("json");
        assert_eq!(
            serde_json::from_str::<Session>(&json).expect("session"),
            session
        );
    }

    #[test]
    fn content_is_read_a_span_at_a_time_and_stops_at_its_size() {
        let dir = porter_fake::Scratch::new("porter-sync-content");
        let path = dir.join("f");
        std::fs::write(&path, b"0123456789").expect("write");
        let content = Content::new(path, 8);
        assert_eq!(content.read(2, 3).expect("read"), b"234");
        assert_eq!(content.read(6, 10).expect("read"), b"67");
        assert_eq!(content.read(9, 1).expect("read"), b"");
    }
}
