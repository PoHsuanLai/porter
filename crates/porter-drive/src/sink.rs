//! Where a drive's changes go: porter-sync's whole-drive writes, one step at a time.

use crate::source::ReplicaSource;
use porter_sync::{BaseVersion, DriveWrite, Place, PutRefused, RemoteId, Session, Upload, Written};
use std::future::Future;
use std::pin::Pin;

/// A write under way.
pub type Sending<'a, T> = Pin<Box<dyn Future<Output = Result<T, PutRefused>> + Send + 'a>>;

/// The writes of a drive, as the core sends its changes: porter-sync's [`DriveWrite`], boxed
/// so a drive holds any of them.
pub trait Sink: Send + Sync {
    /// Whether the drive takes `name` for a file or folder.
    fn accepts_name(&self, name: &str) -> bool;

    /// Makes a folder at `place`.
    fn make_folder<'a>(&'a self, place: &'a Place) -> Sending<'a, Written>;

    /// Sends a file's bytes.
    fn upload<'a>(&'a self, upload: Upload<'a>) -> Sending<'a, Written>;

    /// Renames and/or moves `item` to `to`, if the drive still has `base`.
    fn relocate<'a>(
        &'a self,
        item: &'a RemoteId,
        base: &'a BaseVersion,
        to: &'a Place,
    ) -> Sending<'a, Written>;

    /// Sends `item` to the drive's recycle bin, if the drive still has `base`.
    fn trash<'a>(&'a self, item: &'a RemoteId, base: &'a BaseVersion) -> Sending<'a, ()>;

    /// Gives up an upload session that will not be resumed.
    fn abandon<'a>(&'a self, session: &'a Session)
    -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

impl<R: DriveWrite> Sink for ReplicaSource<R> {
    fn accepts_name(&self, name: &str) -> bool {
        self.0.accepts_name(name)
    }

    fn make_folder<'a>(&'a self, place: &'a Place) -> Sending<'a, Written> {
        Box::pin(self.0.make_folder(place))
    }

    fn upload<'a>(&'a self, upload: Upload<'a>) -> Sending<'a, Written> {
        Box::pin(self.0.upload(upload))
    }

    fn relocate<'a>(
        &'a self,
        item: &'a RemoteId,
        base: &'a BaseVersion,
        to: &'a Place,
    ) -> Sending<'a, Written> {
        Box::pin(self.0.relocate(item, base, to))
    }

    fn trash<'a>(&'a self, item: &'a RemoteId, base: &'a BaseVersion) -> Sending<'a, ()> {
        Box::pin(self.0.trash(item, base))
    }

    fn abandon<'a>(
        &'a self,
        session: &'a Session,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(self.0.abandon(session))
    }
}
