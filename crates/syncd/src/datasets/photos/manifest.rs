//! The metadata manifest: what the person says about each photo (name, favourite, caption,
//! albums, deletion), every field stamped by the clock of the device that wrote it. Two
//! manifests merge field by field, the later stamp winning, so the merge is commutative,
//! associative and idempotent: devices converge whatever order they exchange manifests in, and
//! no two writes of one field ever conflict.

use super::cas::ContentId;
use super::hlc::Hlc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A yes or no a field holds (a favourite, a deletion, an album membership).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// It is.
    On,
    /// It is not.
    Off,
}

/// A value and the stamp of the write that made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamped<T> {
    /// When and where it was written.
    pub at: Hlc,
    /// What was written.
    pub value: T,
}

/// The later of two writes of one field (stamps are unique per device, so a tie is the same
/// write).
fn later<T: Clone>(a: &Option<Stamped<T>>, b: &Option<Stamped<T>>) -> Option<Stamped<T>> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.at > x.at { y } else { x }.clone()),
        (Some(x), None) => Some(x.clone()),
        (None, other) => other.clone(),
    }
}

/// Everything said about one photo.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PhotoMeta {
    /// The file name it was imported as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Stamped<String>>,
    /// Whether it is a favourite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favourite: Option<Stamped<Mark>>,
    /// Its caption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<Stamped<String>>,
    /// Whether it is deleted (the original stays until a purge: deleting is a mark, not a loss).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Stamped<Mark>>,
    /// Album membership, one stamped mark per album, so a photo added to and removed from an
    /// album on two devices settles per album.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub albums: BTreeMap<String, Stamped<Mark>>,
}

impl PhotoMeta {
    fn merged(&self, other: &Self) -> Self {
        let mut albums = self.albums.clone();
        for (album, theirs) in &other.albums {
            let ours = albums.get(album).cloned();
            albums.insert(
                album.clone(),
                later(&ours, &Some(theirs.clone())).unwrap_or_else(|| theirs.clone()),
            );
        }
        Self {
            name: later(&self.name, &other.name),
            favourite: later(&self.favourite, &other.favourite),
            caption: later(&self.caption, &other.caption),
            deleted: later(&self.deleted, &other.deleted),
            albums,
        }
    }

    fn stamps(&self) -> impl Iterator<Item = &Hlc> {
        [
            self.name.as_ref().map(|s| &s.at),
            self.favourite.as_ref().map(|s| &s.at),
            self.caption.as_ref().map(|s| &s.at),
            self.deleted.as_ref().map(|s| &s.at),
        ]
        .into_iter()
        .flatten()
        .chain(self.albums.values().map(|s| &s.at))
    }
}

/// One change to one photo's metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// The imported file name.
    Name(ContentId, String),
    /// Favourite or not.
    Favourite(ContentId, Mark),
    /// The caption (empty clears it).
    Caption(ContentId, String),
    /// In or out of an album.
    Album(ContentId, String, Mark),
    /// Deleted or restored.
    Deleted(ContentId, Mark),
}

/// What the manifest says of one photo, with the defaults of a field nobody wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    /// The imported file name, when anyone recorded it.
    pub name: Option<String>,
    /// Favourite.
    pub favourite: Mark,
    /// Caption, empty when none.
    pub caption: String,
    /// Deleted.
    pub deleted: Mark,
    /// The albums it is in.
    pub albums: BTreeSet<String>,
}

/// Metadata of every photo, by content id.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// The photos with something said about them.
    pub photos: BTreeMap<ContentId, PhotoMeta>,
}

impl Manifest {
    /// Both manifests' writes, the later of each field.
    pub fn merged(&self, other: &Self) -> Self {
        let mut photos = self.photos.clone();
        for (id, theirs) in &other.photos {
            let merged = photos
                .get(id)
                .map_or_else(|| theirs.clone(), |ours| ours.merged(theirs));
            photos.insert(id.clone(), merged);
        }
        Self { photos }
    }

    /// Applies `edit` as written at `at`. An edit older than what is held changes nothing.
    pub fn apply(&mut self, edit: Edit, at: Hlc) {
        let (id, one) = match edit {
            Edit::Name(id, name) => (
                id,
                PhotoMeta {
                    name: Some(Stamped { at, value: name }),
                    ..PhotoMeta::default()
                },
            ),
            Edit::Favourite(id, mark) => (
                id,
                PhotoMeta {
                    favourite: Some(Stamped { at, value: mark }),
                    ..PhotoMeta::default()
                },
            ),
            Edit::Caption(id, text) => (
                id,
                PhotoMeta {
                    caption: Some(Stamped { at, value: text }),
                    ..PhotoMeta::default()
                },
            ),
            Edit::Deleted(id, mark) => (
                id,
                PhotoMeta {
                    deleted: Some(Stamped { at, value: mark }),
                    ..PhotoMeta::default()
                },
            ),
            Edit::Album(id, album, mark) => (
                id,
                PhotoMeta {
                    albums: BTreeMap::from([(album, Stamped { at, value: mark })]),
                    ..PhotoMeta::default()
                },
            ),
        };
        let merged = self
            .photos
            .get(&id)
            .map_or_else(|| one.clone(), |held| held.merged(&one));
        self.photos.insert(id, merged);
    }

    /// What is said of `id`, or `None` when nothing is.
    pub fn photo(&self, id: &ContentId) -> Option<Photo> {
        let meta = self.photos.get(id)?;
        Some(Photo {
            name: meta.name.as_ref().map(|s| s.value.clone()),
            favourite: meta.favourite.as_ref().map_or(Mark::Off, |s| s.value),
            caption: meta
                .caption
                .as_ref()
                .map_or_else(String::new, |s| s.value.clone()),
            deleted: meta.deleted.as_ref().map_or(Mark::Off, |s| s.value),
            albums: meta
                .albums
                .iter()
                .filter(|(_, s)| s.value == Mark::On)
                .map(|(album, _)| album.clone())
                .collect(),
        })
    }

    /// The latest stamp in it.
    pub fn latest(&self) -> Option<&Hlc> {
        self.photos.values().flat_map(PhotoMeta::stamps).max()
    }
}

#[cfg(test)]
mod tests {
    use super::super::hlc::DeviceId;
    use super::*;

    fn at(wall: u64, device: &str) -> Hlc {
        Hlc::new(wall, 0, DeviceId::parse(device).expect("device"))
    }

    fn id(n: u8) -> ContentId {
        ContentId::of(&[n])
    }

    fn manifest(edits: Vec<(Edit, Hlc)>) -> Manifest {
        let mut manifest = Manifest::default();
        for (edit, stamp) in edits {
            manifest.apply(edit, stamp);
        }
        manifest
    }

    #[test]
    fn the_later_write_of_a_field_wins_whatever_order_it_arrives_in() {
        let older = manifest(vec![(Edit::Favourite(id(1), Mark::On), at(100, "a"))]);
        let newer = manifest(vec![(Edit::Favourite(id(1), Mark::Off), at(200, "b"))]);
        for merged in [older.merged(&newer), newer.merged(&older)] {
            assert_eq!(merged.photo(&id(1)).expect("photo").favourite, Mark::Off);
        }
        // Same wall time: the device name settles it, the same on both sides.
        let a = manifest(vec![(Edit::Caption(id(1), "from a".into()), at(100, "a"))]);
        let b = manifest(vec![(Edit::Caption(id(1), "from b".into()), at(100, "b"))]);
        assert_eq!(a.merged(&b), b.merged(&a));
        assert_eq!(a.merged(&b).photo(&id(1)).expect("photo").caption, "from b");
    }

    #[test]
    fn fields_merge_one_by_one_so_a_caption_and_a_favourite_both_survive() {
        let a = manifest(vec![(Edit::Caption(id(1), "beach".into()), at(100, "a"))]);
        let b = manifest(vec![(Edit::Favourite(id(1), Mark::On), at(50, "b"))]);
        let photo = a.merged(&b).photo(&id(1)).expect("photo");
        assert_eq!(
            (photo.caption.as_str(), photo.favourite),
            ("beach", Mark::On)
        );
    }

    #[test]
    fn albums_settle_per_album_and_a_removal_is_a_stamped_mark() {
        let a = manifest(vec![
            (Edit::Album(id(1), "Trip".into(), Mark::On), at(100, "a")),
            (Edit::Album(id(1), "Cats".into(), Mark::On), at(101, "a")),
        ]);
        let b = manifest(vec![
            (Edit::Album(id(1), "Trip".into(), Mark::Off), at(150, "b")),
            (Edit::Album(id(1), "Dogs".into(), Mark::On), at(150, "b")),
        ]);
        let albums = a.merged(&b).photo(&id(1)).expect("photo").albums;
        assert_eq!(
            albums,
            BTreeSet::from(["Cats".to_owned(), "Dogs".to_owned()])
        );
    }

    #[test]
    fn an_edit_older_than_what_is_held_changes_nothing() {
        let mut m = manifest(vec![(Edit::Deleted(id(1), Mark::On), at(200, "a"))]);
        m.apply(Edit::Deleted(id(1), Mark::Off), at(100, "b"));
        assert_eq!(m.photo(&id(1)).expect("photo").deleted, Mark::On);
        m.apply(Edit::Deleted(id(1), Mark::Off), at(300, "b"));
        assert_eq!(m.photo(&id(1)).expect("photo").deleted, Mark::Off);
    }

    #[test]
    fn merging_is_commutative_associative_and_idempotent() {
        let a = manifest(vec![
            (Edit::Name(id(1), "a.jpg".into()), at(1, "a")),
            (Edit::Favourite(id(1), Mark::On), at(5, "a")),
            (Edit::Album(id(2), "x".into(), Mark::On), at(6, "a")),
        ]);
        let b = manifest(vec![
            (Edit::Favourite(id(1), Mark::Off), at(7, "b")),
            (Edit::Caption(id(2), "two".into()), at(8, "b")),
            (Edit::Deleted(id(3), Mark::On), at(2, "b")),
        ]);
        let c = manifest(vec![
            (Edit::Album(id(2), "x".into(), Mark::Off), at(9, "c")),
            (Edit::Caption(id(2), "deux".into()), at(8, "c")),
        ]);
        assert_eq!(a.merged(&b), b.merged(&a));
        assert_eq!(a.merged(&b).merged(&c), a.merged(&b.merged(&c)));
        assert_eq!(a.merged(&c).merged(&b), c.merged(&b).merged(&a));
        assert_eq!(a.merged(&a), a);
        assert_eq!(a.merged(&b).merged(&b), a.merged(&b));
    }

    #[test]
    fn a_manifest_round_trips_json_and_reports_its_latest_stamp() {
        let m = manifest(vec![
            (Edit::Name(id(1), "a.jpg".into()), at(1, "a")),
            (Edit::Album(id(1), "x".into(), Mark::On), at(9, "b")),
        ]);
        let json = serde_json::to_string(&m).expect("json");
        assert_eq!(
            serde_json::from_str::<Manifest>(&json).expect("manifest"),
            m
        );
        assert_eq!(m.latest(), Some(&at(9, "b")));
        assert_eq!(Manifest::default().latest(), None);
        assert_eq!(m.photo(&id(9)), None);
        let defaults = manifest(vec![(Edit::Name(id(4), "n".into()), at(1, "a"))]);
        let photo = defaults.photo(&id(4)).expect("photo");
        assert_eq!(
            (photo.favourite, photo.deleted, photo.caption.as_str()),
            (Mark::Off, Mark::Off, "")
        );
    }
}
