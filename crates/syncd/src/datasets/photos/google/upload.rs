//! [`UploadReplica`]: porter-sync's `Replica` for an upload-only target. The engine runs it over
//! a [`crate::datasets::storage::FolderDataset`] of the upload folder, so the usual machinery
//! (the journal, retries, the scheduler, `Sync1` status and pause) carries the uploads, and the
//! replica is what makes them one-way:
//!
//! - `changes` is always an empty page: nothing is pulled from the library, not even what the
//!   app itself put there.
//! - `put` sends the file unless its SHA-256 is in the [`Ledger`], in which case it answers as if
//!   it had (the file is a copy of something already there). The first upload makes the album.
//! - `remove` is acknowledged without a request: a local delete is not a delete in Google Photos
//!   (the append-only scope cannot do it), and the ledger keeps the hash, so putting the same
//!   file back does not send it again.
//!
//! The remote id of an item is its media item's id; a second path with content already sent gets
//! `<media id>~<path>`, so no two paths share an id in the engine's journal. The version is the
//! content's SHA-256.

use super::api::{AlbumId, ApiError, MediaId, PhotosApi};
use super::ledger::{Entry, Kept, LedgerError};
use crate::dataset::fingerprint;
use porter_core::Bytes;
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_http::Http;
use porter_sync::{
    Anchor, BaseVersion, Blob, ByteRange, ChangePage, Cursor, More, PutItem, PutRefused, PutTarget,
    Quota, RemoteId, RemoteVersion, Replica, ReplicaError, RetryAfter,
};
use std::path::PathBuf;
use storage_webdav::DELETED;
use tokio::sync::Mutex;

/// The dataset's name.
pub const SLUG: &str = "google_photos_upload";

/// The album the uploads go into, by title.
pub const ALBUM_TITLE: &str = "Quire";

/// The anchor of a feed that never has anything.
const NOTHING: &str = "upload-only";

/// How long to wait when Google did not say.
const RETRY: RetryAfter = RetryAfter(60);

/// The replica of the upload folder as an upload target.
#[derive(Debug)]
pub struct UploadReplica<H> {
    api: PhotosApi<H>,
    title: String,
    kept: Mutex<Kept>,
}

fn refusal(error: ApiError) -> PutRefused {
    match error {
        ApiError::Unreached | ApiError::Unreadable => PutRefused::Transient(RetryAfter(30)),
        ApiError::Refused {
            status,
            retry_after,
        } => match status {
            429 | 500..=599 => PutRefused::Transient(retry_after.map_or(RETRY, RetryAfter)),
            _ => PutRefused::Forbidden,
        },
        ApiError::ItemFailed { .. } => PutRefused::Forbidden,
    }
}

fn unkept(_: LedgerError) -> PutRefused {
    PutRefused::Transient(RetryAfter(30))
}

/// The name a path's file is sent under.
fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl<H: Http> UploadReplica<H> {
    /// The replica sending through `api`, keeping its ledger at `ledger`, and putting uploads in
    /// the album titled `title`.
    pub fn open(api: PhotosApi<H>, ledger: PathBuf, title: &str) -> Result<Self, LedgerError> {
        Ok(Self {
            api,
            title: title.to_owned(),
            kept: Mutex::new(Kept::open(ledger)?),
        })
    }

    /// The album, made on first need.
    async fn album(&self, kept: &mut Kept) -> Result<AlbumId, PutRefused> {
        if let Some(album) = &kept.ledger.album {
            return Ok(album.clone());
        }
        let album = self.api.create_album(&self.title).await.map_err(refusal)?;
        kept.ledger.album = Some(album.clone());
        kept.save().map_err(unkept)?;
        Ok(album)
    }

    /// Sends `bytes` under `name` into the album.
    async fn send(
        &self,
        kept: &mut Kept,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<MediaId, PutRefused> {
        let album = self.album(kept).await?;
        let token = self.api.upload(name, bytes).await.map_err(refusal)?;
        match self.api.add_item(Some(&album), name, &token).await {
            Ok(media) => Ok(media),
            Err(ApiError::Refused {
                status: 400 | 404, ..
            }) => {
                // The album is gone (the person deleted it): make a new one next time.
                kept.ledger.album = None;
                kept.save().map_err(unkept)?;
                Err(PutRefused::Transient(RetryAfter(1)))
            }
            Err(error) => Err(refusal(error)),
        }
    }
}

impl<H: Http> Replica for UploadReplica<H> {
    async fn changes(&self, _from: Cursor) -> Result<ChangePage, ReplicaError> {
        Ok(ChangePage {
            changes: Vec::new(),
            next: Anchor(NOTHING.to_owned()),
            more: More::Done,
        })
    }

    async fn fetch(&self, _item: &RemoteId, _range: ByteRange) -> Result<Blob, ReplicaError> {
        // Nothing is read back from the library.
        Err(ReplicaError::Gone)
    }

    async fn put(
        &self,
        item: PutItem,
        _base: BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        let sha = fingerprint(&item.content.0).0;
        let mut kept = self.kept.lock().await;
        let (name, path) = match &item.target {
            PutTarget::New(path) => (name_of(&path.0).to_owned(), Some(path.0.clone())),
            PutTarget::Existing(id) => {
                let media = id.0.split('~').next().unwrap_or(&id.0);
                let name = kept
                    .ledger
                    .of_media(media)
                    .map_or("upload", |e| e.name.as_str());
                (name.to_owned(), None)
            }
        };
        let version = RemoteVersion(sha.clone());
        if let Some(entry) = kept.ledger.find(&sha) {
            // Already there: a copy under another path gets an id of its own.
            let id = match (&item.target, path) {
                (PutTarget::Existing(id), _) => id.0.clone(),
                (PutTarget::New(_), Some(path)) => format!("{}~{path}", entry.media),
                (PutTarget::New(_), None) => entry.media.clone(),
            };
            return Ok((RemoteId(id), version));
        }
        let media = self.send(&mut kept, &name, item.content.0).await?;
        kept.ledger.entries.push(Entry {
            sha256: sha,
            media: media.0.clone(),
            name,
        });
        kept.save().map_err(unkept)?;
        Ok((RemoteId(media.0), version))
    }

    async fn remove(
        &self,
        _item: &RemoteId,
        _base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        Ok(RemoteVersion(DELETED.to_owned()))
    }

    async fn quota(&self) -> Result<Quota, ReplicaError> {
        Ok(Quota {
            used: Bytes(0),
            total: None,
        })
    }

    fn features(&self) -> StorageCap {
        StorageCap {
            access: Access::ReadWrite,
            delta: Delta::None,
            quota: QuotaReport::Unreported,
            scope: StorageScope::AppFolder,
            hashes: HashKind::Sha256,
            ranges: Offered::Absent,
            chunked_upload: Offered::Absent,
        }
    }
}
