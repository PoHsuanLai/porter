//! What a replica whose feed names items by id and parent id (Graph, Drive) has learnt of the
//! tree: each folder's name and parent, and each file's parent. Paths are built from it, and a
//! deleted folder, which the feed reports alone, is known by it to have held its files.
//!
//! A replica keeps one in memory. A dataset that outlives the process takes a snapshot after each
//! page it applies, stores it beside the anchor, and hands it back to the next replica, so a
//! restart neither asks for every folder again nor forgets what a folder deleted meanwhile held.

use crate::item::RemoteId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many folders a chain may have before a walk up it gives up.
const DEPTH: usize = 64;

/// One folder seen.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Folder {
    /// Its name.
    pub name: String,
    /// Its parent's id; none for a drive's root.
    pub parent: Option<RemoteId>,
}

/// The folders and files a deleted folder held, by the tree as it was known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Held {
    /// The folders inside it, at any depth.
    pub folders: Vec<RemoteId>,
    /// The files inside it, at any depth.
    pub files: Vec<RemoteId>,
}

/// The tree a replica has seen; see the module docs. Its serde form is what a dataset stores.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folders {
    /// Folder id to the folder.
    folders: BTreeMap<RemoteId, Folder>,
    /// File id to its parent's id.
    files: BTreeMap<RemoteId, RemoteId>,
}

impl Folders {
    /// An empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// The folder `id`, if seen.
    pub fn folder(&self, id: &RemoteId) -> Option<&Folder> {
        self.folders.get(id)
    }

    /// Records the folder `id` (a rename or a move replaces what was known).
    pub fn insert_folder(&mut self, id: RemoteId, folder: Folder) {
        self.folders.insert(id, folder);
    }

    /// Records that the file `id` is in `parent`.
    pub fn insert_file(&mut self, id: RemoteId, parent: RemoteId) {
        self.files.insert(id, parent);
    }

    /// Forgets the file `id`; whether it was known.
    pub fn remove_file(&mut self, id: &RemoteId) -> bool {
        self.files.remove(id).is_some()
    }

    /// Whether the file `id` is known.
    pub fn has_file(&self, id: &RemoteId) -> bool {
        self.files.contains_key(id)
    }

    /// The id of the folder `id` is in, a folder's or a file's, as far as the tree knows; none
    /// for a drive's root or an id never seen.
    pub fn parent_of(&self, id: &RemoteId) -> Option<&RemoteId> {
        match self.folders.get(id) {
            Some(folder) => folder.parent.as_ref(),
            None => self.files.get(id),
        }
    }

    /// Forgets `id`. A folder takes what it held with it, which is answered; anything else is
    /// forgotten as a file, and `None` is answered.
    pub fn remove(&mut self, id: &RemoteId) -> Option<Held> {
        if self.folders.remove(id).is_none() {
            self.files.remove(id);
            return None;
        }
        let files: Vec<RemoteId> = self
            .files
            .iter()
            .filter(|(_, parent)| self.below(parent, id))
            .map(|(file, _)| file.clone())
            .collect();
        let folders: Vec<RemoteId> = self
            .folders
            .keys()
            .filter(|folder| self.below(folder, id))
            .cloned()
            .collect();
        for folder in &folders {
            self.folders.remove(folder);
        }
        for file in &files {
            self.files.remove(file);
        }
        Some(Held { folders, files })
    }

    /// How many folders and files are known.
    pub fn len(&self) -> usize {
        self.folders.len() + self.files.len()
    }

    /// Whether nothing is known.
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.files.is_empty()
    }

    /// Whether the folder `at` is `ancestor` or inside it, by the folders seen.
    fn below(&self, at: &RemoteId, ancestor: &RemoteId) -> bool {
        let mut current = Some(at);
        for _ in 0..DEPTH {
            match current {
                Some(id) if id == ancestor => return true,
                Some(id) => current = self.folders.get(id).and_then(|f| f.parent.as_ref()),
                None => return false,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> RemoteId {
        RemoteId(text.into())
    }

    fn folder(name: &str, parent: &str) -> Folder {
        Folder {
            name: name.into(),
            parent: Some(id(parent)),
        }
    }

    fn tree() -> Folders {
        let mut tree = Folders::new();
        tree.insert_folder(id("a"), folder("A", "root"));
        tree.insert_folder(id("b"), folder("B", "a"));
        tree.insert_folder(id("c"), folder("C", "root"));
        tree.insert_file(id("f1"), id("a"));
        tree.insert_file(id("f2"), id("b"));
        tree.insert_file(id("f3"), id("c"));
        tree
    }

    #[test]
    fn a_removed_folder_takes_what_it_held_at_any_depth_and_a_file_takes_itself() {
        let mut tree = tree();
        let held = tree.remove(&id("a")).expect("a folder");
        assert_eq!(
            held,
            Held {
                folders: vec![id("b")],
                files: vec![id("f1"), id("f2")],
            }
        );
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.parent_of(&id("c")), Some(&id("root")));
        assert_eq!(tree.parent_of(&id("f3")), Some(&id("c")));
        assert_eq!(tree.parent_of(&id("f1")), None, "gone with its folder");
        assert_eq!(tree.remove(&id("f3")), None);
        assert!(!tree.has_file(&id("f3")));
        // An id never seen is a file no one knew.
        assert_eq!(tree.remove(&id("unknown")), None);
    }

    #[test]
    fn a_snapshot_round_trips_and_pins_its_form() {
        let mut tree = Folders::new();
        tree.insert_folder(id("a"), folder("A", "root"));
        tree.insert_file(id("f1"), id("a"));
        let json = serde_json::to_string(&tree).expect("json");
        assert_eq!(
            json,
            r#"{"folders":{"a":{"name":"A","parent":"root"}},"files":{"f1":"a"}}"#
        );
        assert_eq!(serde_json::from_str::<Folders>(&json).expect("tree"), tree);
    }
}
