//! Every stored or wire type of porter-sync survives its serde form (the journal stores them).

use porter_core::{Bytes, UnixSeconds};
use porter_sync::{
    Anchor, BaseVersion, Change, ChangePage, Conflict, ConflictRule, ContentHash, Cursor,
    DatasetKind, ItemPath, More, Quota, RemoteId, RemoteItem, RemoteSide, RemoteVersion, Tombstone,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    let json = serde_json::to_string(value).expect("serializes");
    assert_eq!(
        &serde_json::from_str::<T>(&json).expect("deserializes"),
        value,
        "{json}"
    );
}

#[test]
fn journal_values_round_trip() {
    let id = RemoteId("id-1".into());
    let version = RemoteVersion("v1".into());
    round_trip(&Cursor::Start);
    round_trip(&Cursor::At(Anchor("token".into())));
    round_trip(&Quota {
        used: Bytes(10),
        total: Some(Bytes(100)),
    });
    round_trip(&ChangePage {
        changes: vec![
            Change::Upsert(RemoteItem {
                id: id.clone(),
                version: version.clone(),
                path: ItemPath("Originals/a.jpg".into()),
                size: Bytes(3),
                hash: Some(ContentHash("abc".into())),
            }),
            Change::Tombstone(Tombstone {
                id: id.clone(),
                version: version.clone(),
                deleted_at: UnixSeconds(1),
            }),
        ],
        next: Anchor("n".into()),
        more: More::More,
    });
    for remote in [
        RemoteSide::Changed(version.clone()),
        RemoteSide::Deleted(version.clone()),
        RemoteSide::Exists(id.clone(), version.clone()),
    ] {
        round_trip(&Conflict {
            item: id.clone(),
            base: BaseVersion::At(version.clone()),
            remote,
        });
    }
    round_trip(&BaseVersion::Absent);
    for kind in [
        DatasetKind::PhotosOriginals,
        DatasetKind::Settings,
        DatasetKind::Keychain,
    ] {
        round_trip(&kind);
    }
    round_trip(&ConflictRule::KeepBoth);
}
