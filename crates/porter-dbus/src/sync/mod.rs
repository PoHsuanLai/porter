//! syncd's interfaces and vocabulary: the datasets (`Sync`), the drives shown in Files (`Storage`)
//! and, with feature `photos-picker`, the Photos Picker, with the bus names, object paths and keys
//! that go with them. These leave porter with syncd.

mod interface;
mod names;
#[cfg(feature = "photos-picker")]
mod photos_picker;
mod storage;

use crate::Document;
use crate::Unserved;

pub use interface::{SyncProxy, SyncServed, SyncServer};
pub use names::*;
#[cfg(feature = "photos-picker")]
pub use photos_picker::{
    PICKER_ERROR_NO_SUCH_SESSION, PICKER_ERROR_NOT_YET, PICKER_ERROR_PREFIX, PICKER_PICKED,
    PICKER_WAITING, PhotosPickerProxy, PhotosPickerServed, PhotosPickerServer,
};
pub use storage::{
    DRIVE_KEY_ACCOUNT, DRIVE_KEY_LABEL, DRIVE_KEY_LOCAL, DRIVE_KEY_PATH, DRIVE_KEY_PROVIDER,
    DRIVE_KEY_STATE, DRIVE_KEY_TOTAL, DRIVE_KEY_USED, DRIVE_KEY_WORDS, DriveState, DrivesChanged,
    DrivesChangedStream, ITEM_KEY_FRACTION, ITEM_KEY_KEPT, ITEM_KEY_KIND, ITEM_KEY_OTHER,
    ITEM_KEY_STATE, ITEM_KEY_URL, ITEM_KEY_WORDS, ItemDetails, ItemKind, ItemState, ItemsChanged,
    ItemsChangedStream, Refused, Saved, StorageProxy, StorageServed, StorageServer,
};

/// `org.quire.Sync1`, and the Photos Picker beside it only with the `photos-picker` feature.
#[cfg(feature = "photos-picker")]
pub const SYNC: Document = Document {
    file_name: "org.quire.Sync1.xml",
    interfaces: &[
        SyncServed::<Unserved>::introspect,
        PhotosPickerServed::<Unserved>::introspect,
    ],
};
/// `org.quire.Sync1`.
#[cfg(not(feature = "photos-picker"))]
pub const SYNC: Document = Document {
    file_name: "org.quire.Sync1.xml",
    interfaces: &[SyncServed::<Unserved>::introspect],
};
/// `org.quire.Storage1`, on syncd's bus name.
pub const STORAGE: Document = Document {
    file_name: "org.quire.Storage1.xml",
    interfaces: &[StorageServed::<Unserved>::introspect],
};
