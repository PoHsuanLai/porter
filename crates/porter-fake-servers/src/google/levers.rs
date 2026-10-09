//! The test's side of the fake Google's Drive and Photos: another device writing, moving or
//! deleting a file, a person picking photos, and what the fake holds now.

use super::GoogleHandle;
use super::drive::Node;
use super::photos::{Album, MediaItem, Pick, Session};
use crate::seen::lock;

fn names_of(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn path_of(drive: &super::drive::Drive, node: &Node) -> String {
    let mut names = vec![node.name.clone()];
    let mut at = node.parent.as_deref().and_then(|p| drive.any(p));
    while let Some(parent) = at.filter(|p| p.id != drive.root()) {
        names.push(parent.name.clone());
        at = parent.parent.as_deref().and_then(|p| drive.any(p));
    }
    names.reverse();
    names.join("/")
}

impl GoogleHandle {
    /// Accepts this bearer too, as it is (no scope check).
    pub fn accept_bearer(&self, token: &str) {
        lock(&self.shared.state).fixed.push(token.to_owned());
    }

    /// The next `count` requests with a good bearer are answered `429` with this `Retry-After`.
    pub fn throttle(&self, count: u32, retry_after: u32) {
        lock(&self.shared.state).throttle = Some((count, retry_after));
    }

    /// Creates or replaces the file at `path` below the app data folder (folders on the way are
    /// made), as another device of the same account would.
    pub fn drive_put_file(&self, path: &str, bytes: &[u8]) {
        let names = names_of(path);
        let mut state = lock(&self.shared.state);
        let Some((name, folders)) = names.split_last() else {
            return;
        };
        let drive = &mut state.drive;
        let parent = drive
            .ensure(folders)
            .expect("the folders on the path are folders");
        let over = drive
            .children(&parent)
            .into_iter()
            .find(|n| !n.is_folder() && &n.name == name)
            .map(|n| n.id.clone());
        drive
            .write(&parent, name, bytes.to_vec(), over.as_deref())
            .expect("the drive has room");
    }

    /// Deletes the file or folder at `path` permanently.
    pub fn drive_delete(&self, path: &str) {
        let mut state = lock(&self.shared.state);
        if let Some(id) = state.drive.at(&names_of(path)).map(|n| n.id.clone()) {
            state.drive.delete(&id);
        }
    }

    /// Puts the file or folder at `path` in the trash.
    pub fn drive_trash(&self, path: &str) {
        let mut state = lock(&self.shared.state);
        if let Some(id) = state.drive.at(&names_of(path)).map(|n| n.id.clone()) {
            state.drive.trash(&id, true);
        }
    }

    /// Moves or renames the item at `from` to the path `to` (the folders on the way are made).
    pub fn drive_move(&self, from: &str, to: &str) {
        let target = names_of(to);
        let mut state = lock(&self.shared.state);
        let Some((name, folders)) = target.split_last() else {
            return;
        };
        let Some(id) = state.drive.at(&names_of(from)).map(|n| n.id.clone()) else {
            return;
        };
        let parent = state
            .drive
            .ensure(folders)
            .expect("the folders on the path are folders");
        state.drive.relocate(&id, Some(name), Some(&parent));
    }

    /// The bytes of the file at `path`.
    pub fn drive_file(&self, path: &str) -> Option<Vec<u8>> {
        lock(&self.shared.state)
            .drive
            .at(&names_of(path))
            .and_then(|n| n.bytes.clone())
    }

    /// The `version` of the item at `path`.
    pub fn drive_version(&self, path: &str) -> Option<String> {
        lock(&self.shared.state)
            .drive
            .at(&names_of(path))
            .map(|n| n.version.to_string())
    }

    /// Every live file: its path and size, in path order.
    pub fn drive_files(&self) -> Vec<(String, u64)> {
        let state = lock(&self.shared.state);
        let mut found: Vec<(String, u64)> = state
            .drive
            .live()
            .iter()
            .filter(|n| !n.is_folder())
            .map(|n| {
                (
                    path_of(&state.drive, n),
                    n.bytes.as_ref().map_or(0, Vec::len) as u64,
                )
            })
            .collect();
        found.sort();
        found
    }

    /// Every page token handed out so far is dropped.
    pub fn drive_expire_tokens(&self) {
        lock(&self.shared.state).drive.expire_tokens();
    }

    /// Gives the drive room for `total` bytes.
    pub fn drive_set_limit(&self, total: u64) {
        lock(&self.shared.state).drive.set_limit(total);
    }

    /// What uploads to Photos made, oldest first.
    pub fn photos_items(&self) -> Vec<MediaItem> {
        lock(&self.shared.state).photos.items.clone()
    }

    /// The albums created, oldest first.
    pub fn photos_albums(&self) -> Vec<Album> {
        lock(&self.shared.state).photos.albums.clone()
    }

    /// The Picker sessions, oldest first (a deleted one stays, marked).
    pub fn picker_sessions(&self) -> Vec<Session> {
        lock(&self.shared.state).photos.sessions.clone()
    }

    /// The person finishes picking in `session`: these are what they picked. `false` when the
    /// session is unknown or deleted.
    pub fn picker_pick(&self, session: &str, picks: Vec<Pick>) -> bool {
        lock(&self.shared.state).photos.pick(session, picks)
    }
}
