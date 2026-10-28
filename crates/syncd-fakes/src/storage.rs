//! A fake `org.quire.Storage1` for building Files before syncd serves the real one: two drives
//! of real files under a scratch folder, whose states move as the real ones will. Download Now
//! fills a progress fraction ten times a second until the file is downloaded; Keep Downloaded and
//! Remove Download change what they say; the commands of [`World::command`] (read from stdin by
//! `porter-rig-storage`) take a drive offline, ask for sign-in again, add a file as another
//! device would, or start an upload, an error or a conflict on one item.
//!
//! The files on disk are ordinary files with a line of text: nothing here is downloaded from
//! anywhere, only the states are. Test tooling, never installed.

use porter_dbus::sync::STORAGE_PATH;
use porter_dbus::sync::{
    DRIVE_KEY_ACCOUNT, DRIVE_KEY_LABEL, DRIVE_KEY_LOCAL, DRIVE_KEY_PATH, DRIVE_KEY_PROVIDER,
    DRIVE_KEY_STATE, DRIVE_KEY_TOTAL, DRIVE_KEY_USED, DRIVE_KEY_WORDS, DriveState,
    ITEM_KEY_FRACTION, ITEM_KEY_KEPT, ITEM_KEY_KIND, ITEM_KEY_OTHER, ITEM_KEY_STATE, ITEM_KEY_URL,
    ITEM_KEY_WORDS, ItemDetails, ItemKind, ItemState, Refused, Saved, StorageServed, StorageServer,
};
use porter_dbus::{Call, Details};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{Connection, fdo};

/// How often a moving transfer steps, and so how often `ItemsChanged` is sent while one moves.
pub const TICK: Duration = Duration::from_millis(100);

/// How far one tick moves a transfer: twenty ticks, two seconds, from nothing to done.
const STEP: f64 = 0.05;

/// The words a refused action gives, as Files may show them: the same as the live service
/// (`syncd::words::reason`; a test in syncd holds the two together).
pub mod words {
    /// A path on no drive.
    pub const NOT_ON_A_DRIVE: &str = "This item isn't in a cloud drive.";
    /// An online document asked to download, keep or remove.
    pub const ONLINE_ONLY: &str = "This document opens online.";
    /// A download asked for while the drive cannot be reached.
    pub const OFFLINE: &str = "This file will download when you're connected.";
    /// A removal asked for an item with changes not sent yet.
    pub const WAITING: &str = "Waiting to upload.";
    /// A removal asked for an item with two versions.
    pub const CONFLICT: &str = "This file has two versions. Choose one first.";
    /// Keep Downloaded off, or a removal, asked for an item inside a kept folder.
    pub const KEPT_WITH_FOLDER: &str = "This item is kept downloaded with its folder.";
    /// A drive id that names no drive.
    pub const NO_DRIVE: &str = "This drive isn't in Files.";
    /// Save a Copy asked for an item that is not an online document.
    pub const NOT_A_DOCUMENT: &str = "Only an online document can be saved as a copy.";
    /// Save a Copy asked for a Form, Site or Map, which have no file format.
    pub const NO_FILE_FORMAT: &str = "This document can't be saved as a file.";
}

/// What a file of the other version of a conflict is called: its name with this before the
/// extension.
const OTHER_VERSION: &str = " (this computer)";

/// One drive of the fake.
#[derive(Debug, Clone)]
struct Drive {
    id: String,
    account: String,
    label: String,
    provider: String,
    path: PathBuf,
    state: DriveState,
    words: Option<String>,
    used: u64,
    total: Option<u64>,
}

/// One file or folder of the fake. A folder's state is worked out from what is under it; its
/// `kept` is its own, and everything under it reads kept with it.
#[derive(Debug, Clone)]
struct Item {
    drive: String,
    kind: ItemKind,
    state: ItemState,
    kept: bool,
    fraction: f64,
    words: Option<String>,
    /// Bytes, as the drive says: what downloading it takes here.
    size: u64,
    /// An online document's address.
    url: Option<String>,
    /// The other version of a file in conflict.
    other: Option<PathBuf>,
}

/// What a step changed: per drive, the paths whose details are new.
pub type Changed = BTreeMap<String, BTreeSet<PathBuf>>;

/// The fake's drives and items.
#[derive(Debug)]
pub struct World {
    drives: Vec<Drive>,
    items: BTreeMap<PathBuf, Item>,
}

/// Each file the fake plants: its drive, its path under the drive, its kind, its state, whether
/// it is kept, and its size.
const PLANTED: &[(&str, &str, ItemKind, ItemState, bool, u64)] = &[
    (
        "onedrive",
        "Documents",
        ItemKind::Folder,
        ItemState::Cloud,
        false,
        0,
    ),
    (
        "onedrive",
        "Documents/Report.docx",
        ItemKind::File,
        ItemState::Cloud,
        false,
        240_000,
    ),
    (
        "onedrive",
        "Documents/Notes.txt",
        ItemKind::File,
        ItemState::Downloaded,
        false,
        4_000,
    ),
    (
        "onedrive",
        "Documents/Taxes 2025.pdf",
        ItemKind::File,
        ItemState::Downloaded,
        true,
        1_800_000,
    ),
    (
        "onedrive",
        "Pictures",
        ItemKind::Folder,
        ItemState::Cloud,
        false,
        0,
    ),
    (
        "onedrive",
        "Pictures/Beach.jpg",
        ItemKind::File,
        ItemState::Cloud,
        false,
        3_600_000,
    ),
    (
        "onedrive",
        "Pictures/Garden.jpg",
        ItemKind::File,
        ItemState::Waiting,
        false,
        2_900_000,
    ),
    (
        "onedrive",
        "Budget.xlsx",
        ItemKind::File,
        ItemState::Conflict,
        false,
        52_000,
    ),
    (
        "gdrive",
        "Projects",
        ItemKind::Folder,
        ItemState::Cloud,
        false,
        0,
    ),
    (
        "gdrive",
        "Projects/Plan.gdoc",
        ItemKind::Online,
        ItemState::Cloud,
        false,
        0,
    ),
    (
        "gdrive",
        "Projects/Survey.gform",
        ItemKind::Online,
        ItemState::Cloud,
        false,
        0,
    ),
    (
        "gdrive",
        "Projects/Sketch.png",
        ItemKind::File,
        ItemState::Cloud,
        false,
        760_000,
    ),
    (
        "gdrive",
        "Projects/Old draft.odt",
        ItemKind::File,
        ItemState::Error,
        false,
        31_000,
    ),
    (
        "gdrive",
        "Recipes.pdf",
        ItemKind::File,
        ItemState::Downloaded,
        false,
        980_000,
    ),
];

impl World {
    /// Two drives under `dir/drives/`, a OneDrive and a Google Drive, their files written to
    /// disk.
    pub fn plant(dir: &Path) -> std::io::Result<World> {
        let root = dir.join("drives");
        let drives = vec![
            Drive {
                id: "onedrive".into(),
                account: "/org/quire/Accounts1/account/fake_onedrive".into(),
                label: "ada@example.test".into(),
                provider: "OneDrive".into(),
                path: root.join("OneDrive"),
                state: DriveState::Ready,
                words: None,
                used: 3_400_000_000,
                total: Some(5_000_000_000),
            },
            Drive {
                id: "gdrive".into(),
                account: "/org/quire/Accounts1/account/fake_gdrive".into(),
                label: "ada@example.test".into(),
                provider: "Google Drive".into(),
                path: root.join("Google Drive"),
                state: DriveState::Ready,
                words: None,
                used: 1_200_000_000,
                total: Some(15_000_000_000),
            },
        ];
        for drive in &drives {
            std::fs::create_dir_all(&drive.path)?;
        }
        let mut world = World {
            drives,
            items: BTreeMap::new(),
        };
        for (id, under, kind, state, kept, size) in PLANTED {
            let path = world
                .drives
                .iter()
                .find(|drive| drive.id == *id)
                .expect("a planted item's drive is one of the two")
                .path
                .join(under);
            let url = (*kind == ItemKind::Online).then(|| online_url(&path));
            match (kind, &url) {
                (ItemKind::Folder, _) => std::fs::create_dir_all(&path)?,
                (ItemKind::Online, Some(url)) => std::fs::write(&path, link_file(url))?,
                _ => std::fs::write(&path, format!("A fake file: {under}\n"))?,
            }
            let words =
                (*state == ItemState::Error).then(|| "The server could not send it.".into());
            world.items.insert(
                path.clone(),
                Item {
                    drive: (*id).into(),
                    kind: *kind,
                    state: *state,
                    kept: *kept,
                    fraction: 0.0,
                    words,
                    size: *size,
                    url,
                    other: None,
                },
            );
            if *state == ItemState::Conflict {
                world.split(&path, &mut Changed::new())?;
            }
        }
        Ok(world)
    }

    /// The drive rows, as `Drives` answers.
    pub fn drives(&self) -> Vec<(String, Details)> {
        self.drives
            .iter()
            .map(|drive| {
                let mut details = Details::new();
                put(&mut details, DRIVE_KEY_ACCOUNT, object(&drive.account));
                put(
                    &mut details,
                    DRIVE_KEY_LABEL,
                    Value::from(drive.label.as_str()),
                );
                put(
                    &mut details,
                    DRIVE_KEY_PROVIDER,
                    Value::from(drive.provider.as_str()),
                );
                put(
                    &mut details,
                    DRIVE_KEY_PATH,
                    Value::from(drive.path.to_string_lossy().as_ref()),
                );
                put(
                    &mut details,
                    DRIVE_KEY_STATE,
                    Value::from(drive.state.as_str()),
                );
                put(&mut details, DRIVE_KEY_USED, Value::U64(drive.used));
                put(
                    &mut details,
                    DRIVE_KEY_LOCAL,
                    Value::U64(self.local(&drive.id)),
                );
                if let Some(total) = drive.total {
                    put(&mut details, DRIVE_KEY_TOTAL, Value::U64(total));
                }
                if let Some(words) = &drive.words {
                    put(&mut details, DRIVE_KEY_WORDS, Value::from(words.as_str()));
                }
                (drive.id.clone(), details)
            })
            .collect()
    }

    /// Bytes of a drive's files here: what is downloaded, or changed here and not sent yet.
    fn local(&self, drive: &str) -> u64 {
        self.items
            .values()
            .filter(|item| item.drive == drive && item.kind == ItemKind::File)
            .filter(|item| {
                matches!(
                    item.state,
                    ItemState::Downloaded
                        | ItemState::Uploading
                        | ItemState::Waiting
                        | ItemState::Conflict
                )
            })
            .map(|item| item.size)
            .sum()
    }

    /// Every drive's bytes here, to tell whether a step changed the drive rows.
    fn locals(&self) -> Vec<u64> {
        self.drives
            .iter()
            .map(|drive| self.local(&drive.id))
            .collect()
    }

    /// Each known path's details, as `Items` answers; unknown paths are left out.
    pub fn items(&self, paths: &[String]) -> ItemDetails {
        paths
            .iter()
            .filter_map(|text| {
                let path = PathBuf::from(text);
                self.details(&path).map(|details| (text.clone(), details))
            })
            .collect()
    }

    /// "Download Now".
    pub fn download(&mut self, paths: &[String], changed: &mut Changed) -> Refused {
        self.each_file(paths, changed, |world, path, changed| {
            world.start_download(path, changed)
        })
    }

    /// "Keep Downloaded" on or off. On, a folder's own flag is set and everything under it,
    /// also what is added later, reads kept; off, everything under it is turned off too.
    pub fn keep(&mut self, paths: &[String], keep: bool, changed: &mut Changed) -> Refused {
        let mut refused = Refused::new();
        for text in paths {
            let path = PathBuf::from(text);
            match self.items.get(&path).map(|item| item.kind) {
                None => {
                    refused.insert(text.clone(), words::NOT_ON_A_DRIVE.into());
                    continue;
                }
                Some(ItemKind::Online) => {
                    refused.insert(text.clone(), words::ONLINE_ONLY.into());
                    continue;
                }
                Some(_) => {}
            }
            if !keep && self.kept_by_folder(&path) {
                refused.insert(text.clone(), words::KEPT_WITH_FOLDER.into());
                continue;
            }
            for inner in self.under(&path) {
                if let Some(item) = self.items.get_mut(&inner)
                    && (inner == path || !keep)
                {
                    item.kept = keep;
                }
                self.touch(&inner, changed);
            }
            if keep && let Some(why) = self.download_under(&path, changed) {
                refused.insert(text.clone(), why);
            }
        }
        refused
    }

    /// "Remove Download".
    pub fn remove_download(&mut self, paths: &[String], changed: &mut Changed) -> Refused {
        let mut refused = Refused::new();
        let mut taken = Vec::new();
        for text in paths {
            let path = Path::new(text);
            if self.items.contains_key(path) && self.kept_by_folder(path) {
                refused.insert(text.clone(), words::KEPT_WITH_FOLDER.into());
            } else {
                taken.push(text.clone());
            }
        }
        refused.extend(self.each_file(&taken, changed, |world, path, changed| {
            let item = world.items.get_mut(path)?;
            match item.state {
                ItemState::Waiting | ItemState::Uploading => return Some(words::WAITING.into()),
                ItemState::Conflict => return Some(words::CONFLICT.into()),
                _ => {}
            }
            item.state = ItemState::Cloud;
            item.kept = false;
            item.fraction = 0.0;
            item.words = None;
            world.touch(path, changed);
            None
        }));
        // A folder removed is no longer kept itself.
        for text in &taken {
            if let Some(item) = self.items.get_mut(Path::new(text))
                && item.kind == ItemKind::Folder
            {
                item.kept = false;
            }
        }
        refused
    }

    /// "Save a Copy": each online document's export written next to it as a downloaded file,
    /// "Plan.docx", or "Plan 2.docx", "Plan 3.docx" when the name is taken.
    pub fn save_copy(&mut self, paths: &[String], changed: &mut Changed) -> (Saved, Refused) {
        let mut saved = Saved::new();
        let mut refused = Refused::new();
        for text in paths {
            let path = PathBuf::from(text);
            let Some(item) = self.items.get(&path) else {
                refused.insert(text.clone(), words::NOT_ON_A_DRIVE.into());
                continue;
            };
            if item.kind != ItemKind::Online {
                refused.insert(text.clone(), words::NOT_A_DOCUMENT.into());
                continue;
            }
            let Some(extension) = export_of(&path) else {
                refused.insert(text.clone(), words::NO_FILE_FORMAT.into());
                continue;
            };
            let drive = item.drive.clone();
            let copy = self.free_name(&path, extension);
            let contents = format!("A fake copy of {}\n", path.display());
            if let Err(why) = std::fs::write(&copy, &contents) {
                refused.insert(text.clone(), format!("The copy couldn't be written: {why}"));
                continue;
            }
            self.items.insert(
                copy.clone(),
                Item {
                    drive,
                    kind: ItemKind::File,
                    state: ItemState::Downloaded,
                    kept: false,
                    fraction: 0.0,
                    words: None,
                    size: contents.len() as u64,
                    url: None,
                    other: None,
                },
            );
            self.touch(&copy, changed);
            saved.insert(text.clone(), copy.to_string_lossy().into_owned());
        }
        (saved, refused)
    }

    /// The first name next to `document` with its stem and `extension` that no item or file
    /// has: "Plan.docx", then "Plan 2.docx", "Plan 3.docx".
    fn free_name(&self, document: &Path, extension: &str) -> PathBuf {
        let stem = document
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        (1..)
            .map(|n| match n {
                1 => document.with_file_name(format!("{stem}.{extension}")),
                n => document.with_file_name(format!("{stem} {n}.{extension}")),
            })
            .find(|name| !self.items.contains_key(name) && !name.exists())
            .expect("some number is free")
    }

    /// "Pause Syncing" or "Resume Syncing" for each drive id. Pausing holds what would upload;
    /// resuming sends it.
    pub fn set_paused(
        &mut self,
        drives: &[String],
        paused: bool,
        changed: &mut Changed,
    ) -> Refused {
        let mut refused = Refused::new();
        for id in drives {
            let Some(drive) = self.drives.iter_mut().find(|drive| drive.id == *id) else {
                refused.insert(id.clone(), words::NO_DRIVE.into());
                continue;
            };
            drive.state = if paused {
                DriveState::Paused
            } else {
                DriveState::Ready
            };
            drive.words = None;
            let (from, to) = if paused {
                (ItemState::Uploading, ItemState::Waiting)
            } else {
                (ItemState::Waiting, ItemState::Uploading)
            };
            let moving: Vec<PathBuf> = self
                .items
                .iter()
                .filter(|(_, item)| item.drive == *id && item.state == from)
                .map(|(path, _)| path.clone())
                .collect();
            for path in moving {
                if let Some(item) = self.items.get_mut(&path) {
                    item.state = to;
                    item.fraction = 0.0;
                }
                self.touch(&path, changed);
            }
        }
        refused
    }

    /// Moves every transfer one step: what changed, per drive.
    pub fn tick(&mut self) -> Changed {
        let mut changed = Changed::new();
        let moving: Vec<PathBuf> = self
            .items
            .iter()
            .filter(|(_, item)| matches!(item.state, ItemState::Downloading | ItemState::Uploading))
            .map(|(path, _)| path.clone())
            .collect();
        for path in moving {
            let offline = self
                .drive_of_path(&path)
                .is_some_and(|drive| drive.state == DriveState::Offline);
            let Some(item) = self.items.get_mut(&path) else {
                continue;
            };
            if offline {
                // A transfer cut off by the network: a download goes back to the cloud, an upload
                // waits.
                item.state = match item.state {
                    ItemState::Uploading => ItemState::Waiting,
                    _ => ItemState::Cloud,
                };
                item.fraction = 0.0;
            } else {
                item.fraction = (item.fraction + STEP).min(1.0);
                if item.fraction >= 1.0 {
                    item.state = ItemState::Downloaded;
                    item.fraction = 0.0;
                }
            }
            self.touch(&path, &mut changed);
        }
        changed
    }

    /// One command, as `porter-rig-storage` reads them from stdin; what it changed, and whether
    /// the drives changed. `Err` holds what to print for a command it does not know.
    pub fn command(&mut self, line: &str) -> Result<(Changed, bool), String> {
        let mut changed = Changed::new();
        let mut words_of = line.split_whitespace();
        let verb = words_of.next().unwrap_or_default();
        let rest: Vec<&str> = words_of.collect();
        let drives = match (verb, rest.as_slice()) {
            ("offline", [drive]) => self.set_drive(drive, DriveState::Offline, None)?,
            ("online" | "ready" | "resume", [drive]) => {
                self.known_drive(drive)?;
                self.set_paused(&[(*drive).to_owned()], false, &mut changed);
                true
            }
            ("pause", [drive]) => {
                self.known_drive(drive)?;
                self.set_paused(&[(*drive).to_owned()], true, &mut changed);
                true
            }
            ("signin", [drive]) => self.set_drive(
                drive,
                DriveState::NeedsSignIn,
                Some("Sign in again to see this drive's files.".into()),
            )?,
            ("add", path) if !path.is_empty() => {
                self.add(&joined(path), &mut changed)?;
                false
            }
            ("upload", path) if !path.is_empty() => {
                self.set_item(&joined(path), ItemState::Uploading, None, &mut changed)?;
                false
            }
            ("wait", path) if !path.is_empty() => {
                self.set_item(&joined(path), ItemState::Waiting, None, &mut changed)?;
                false
            }
            ("conflict", path) if !path.is_empty() => {
                let path = joined(path);
                self.set_item(&path, ItemState::Conflict, None, &mut changed)?;
                self.split(&path, &mut changed)
                    .map_err(|why| format!("cannot write the other version: {why}"))?;
                false
            }
            ("error", path) if !path.is_empty() => {
                self.set_item(
                    &joined(path),
                    ItemState::Error,
                    Some("The server could not send it.".into()),
                    &mut changed,
                )?;
                false
            }
            _ => return Err(HELP.into()),
        };
        Ok((changed, drives))
    }

    fn known_drive(&self, id: &str) -> Result<(), String> {
        if self.drives.iter().any(|drive| drive.id == id) {
            Ok(())
        } else {
            Err(format!("no drive {id:?}: onedrive or gdrive"))
        }
    }

    fn set_drive(
        &mut self,
        id: &str,
        state: DriveState,
        words: Option<String>,
    ) -> Result<bool, String> {
        let drive = self
            .drives
            .iter_mut()
            .find(|drive| drive.id == id)
            .ok_or_else(|| format!("no drive {id:?}: onedrive or gdrive"))?;
        drive.state = state;
        drive.words = words;
        Ok(true)
    }

    fn set_item(
        &mut self,
        path: &Path,
        state: ItemState,
        words: Option<String>,
        changed: &mut Changed,
    ) -> Result<(), String> {
        let item = self
            .items
            .get_mut(path)
            .filter(|item| item.kind == ItemKind::File)
            .ok_or_else(|| format!("no file {}", path.display()))?;
        item.state = state;
        item.fraction = 0.0;
        item.words = words;
        self.touch(path, changed);
        Ok(())
    }

    /// A file another device added: listed, not downloaded, and downloading at once when a
    /// folder above it is kept.
    fn add(&mut self, path: &Path, changed: &mut Changed) -> Result<(), String> {
        let drive = self
            .drive_of_path(path)
            .ok_or_else(|| format!("{} is in no drive", path.display()))?
            .id
            .clone();
        let parent = path.parent().unwrap_or(path);
        let in_folder = self
            .items
            .get(parent)
            .is_some_and(|item| item.kind == ItemKind::Folder);
        let at_top = self.drives.iter().any(|drive| drive.path == parent);
        if !(in_folder || at_top) || self.items.contains_key(path) {
            return Err(format!("cannot add {}", path.display()));
        }
        std::fs::write(path, "A fake file another device added\n")
            .map_err(|why| format!("cannot write {}: {why}", path.display()))?;
        self.items.insert(
            path.to_path_buf(),
            Item {
                drive,
                kind: ItemKind::File,
                state: ItemState::Cloud,
                kept: false,
                fraction: 0.0,
                words: None,
                size: 1_000_000,
                url: None,
                other: None,
            },
        );
        self.touch(path, changed);
        if self.kept_by_folder(path) {
            self.start_download(path, changed);
        }
        Ok(())
    }

    /// Keeps both versions of a file in conflict: the other one is written next to it, and each
    /// names the other.
    fn split(&mut self, path: &Path, changed: &mut Changed) -> std::io::Result<()> {
        let Some(item) = self.items.get(path).cloned() else {
            return Ok(());
        };
        if item.other.is_some() {
            return Ok(());
        }
        let other = other_version(path);
        std::fs::write(
            &other,
            "A fake file: the version changed on this computer\n",
        )?;
        self.items.insert(
            other.clone(),
            Item {
                other: Some(path.to_path_buf()),
                kept: false,
                ..item
            },
        );
        if let Some(item) = self.items.get_mut(path) {
            item.other = Some(other.clone());
        }
        self.touch(path, changed);
        self.touch(&other, changed);
        Ok(())
    }

    /// Runs `act` on every file of `paths` (a folder: every file under it), gathering refusals by
    /// the path the caller gave: the first refusal of a folder's files stands for it.
    fn each_file(
        &mut self,
        paths: &[String],
        changed: &mut Changed,
        mut act: impl FnMut(&mut World, &Path, &mut Changed) -> Option<String>,
    ) -> Refused {
        let mut refused = Refused::new();
        for text in paths {
            let path = PathBuf::from(text);
            let Some(item) = self.items.get(&path) else {
                refused.insert(text.clone(), words::NOT_ON_A_DRIVE.into());
                continue;
            };
            if item.kind == ItemKind::Online {
                refused.insert(text.clone(), words::ONLINE_ONLY.into());
                continue;
            }
            let mut why = None;
            for inner in self.files(&path) {
                if let Some(said) = act(self, &inner, changed) {
                    why.get_or_insert(said);
                }
            }
            if let Some(why) = why {
                refused.insert(text.clone(), why);
            }
        }
        refused
    }

    /// Starts a download of every file at or under `path`: the first refusal, if any.
    fn download_under(&mut self, path: &Path, changed: &mut Changed) -> Option<String> {
        let mut why = None;
        for inner in self.files(path) {
            if let Some(said) = self.start_download(&inner, changed) {
                why.get_or_insert(said);
            }
        }
        why
    }

    fn start_download(&mut self, path: &Path, changed: &mut Changed) -> Option<String> {
        let offline = self
            .drive_of_path(path)
            .is_some_and(|drive| drive.state == DriveState::Offline);
        let item = self.items.get_mut(path)?;
        if !matches!(item.state, ItemState::Cloud | ItemState::Error) {
            return None;
        }
        if offline {
            return Some(words::OFFLINE.into());
        }
        item.state = ItemState::Downloading;
        item.fraction = 0.0;
        item.words = None;
        self.touch(path, changed);
        None
    }

    /// `path` and every item under it.
    fn under(&self, path: &Path) -> Vec<PathBuf> {
        self.items
            .keys()
            .filter(|inner| inner.starts_with(path))
            .cloned()
            .collect()
    }

    /// The files at or under `path`.
    fn files(&self, path: &Path) -> Vec<PathBuf> {
        self.items
            .iter()
            .filter(|(inner, item)| inner.starts_with(path) && item.kind == ItemKind::File)
            .map(|(inner, _)| inner.clone())
            .collect()
    }

    /// Whether a folder above `path` is kept.
    fn kept_by_folder(&self, path: &Path) -> bool {
        path.ancestors()
            .skip(1)
            .filter_map(|above| self.items.get(above))
            .any(|item| item.kind == ItemKind::Folder && item.kept)
    }

    /// Marks `path` and every folder above it on its drive as changed.
    fn touch(&self, path: &Path, changed: &mut Changed) {
        let Some(drive) = self.drive_of_path(path) else {
            return;
        };
        let set = changed.entry(drive.id.clone()).or_default();
        let mut at = Some(path);
        while let Some(here) = at {
            if !here.starts_with(&drive.path) || here == drive.path {
                break;
            }
            set.insert(here.to_path_buf());
            at = here.parent();
        }
    }

    fn drive_of_path(&self, path: &Path) -> Option<&Drive> {
        self.drives
            .iter()
            .find(|drive| path.starts_with(&drive.path))
    }

    /// One path's details: a file's own, a folder's worked out from what is under it.
    fn details(&self, path: &Path) -> Option<Details> {
        let item = self.items.get(path)?;
        let (state, fraction) = match item.kind {
            ItemKind::Folder => self.folder_state(path),
            _ => (item.state, item.fraction),
        };
        let kept = item.kept || self.kept_by_folder(path);
        let mut details = Details::new();
        put(&mut details, ITEM_KEY_STATE, Value::from(state.as_str()));
        put(&mut details, ITEM_KEY_KEPT, Value::Bool(kept));
        put(&mut details, ITEM_KEY_KIND, Value::from(item.kind.as_str()));
        if matches!(state, ItemState::Downloading | ItemState::Uploading) {
            put(&mut details, ITEM_KEY_FRACTION, Value::F64(fraction));
        }
        if let (ItemState::Error, Some(words)) = (state, &item.words) {
            put(&mut details, ITEM_KEY_WORDS, Value::from(words.as_str()));
        }
        if let Some(url) = &item.url {
            put(&mut details, ITEM_KEY_URL, Value::from(url.as_str()));
        }
        if let (ItemState::Conflict, Some(other)) = (state, &item.other) {
            put(
                &mut details,
                ITEM_KEY_OTHER,
                Value::from(other.to_string_lossy().as_ref()),
            );
        }
        Some(details)
    }

    fn files_under<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Item> + 'a {
        self.items
            .iter()
            .filter(move |(inner, item)| inner.starts_with(path) && item.kind == ItemKind::File)
            .map(|(_, item)| item)
    }

    /// A folder's state: conflict, error, moving (with the share of the moving bytes that have
    /// moved), downloaded when every file is, else cloud.
    fn folder_state(&self, path: &Path) -> (ItemState, f64) {
        let files: Vec<&Item> = self.files_under(path).collect();
        let any = |state: ItemState| files.iter().any(|item| item.state == state);
        for state in [ItemState::Conflict, ItemState::Error] {
            if any(state) {
                return (state, 0.0);
            }
        }
        for state in [ItemState::Uploading, ItemState::Downloading] {
            let moving: Vec<&&Item> = files.iter().filter(|item| item.state == state).collect();
            if !moving.is_empty() {
                let bytes: u64 = moving.iter().map(|item| item.size).sum();
                let moved: f64 = moving
                    .iter()
                    .map(|item| item.size as f64 * item.fraction)
                    .sum();
                let fraction = if bytes == 0 {
                    0.0
                } else {
                    moved / bytes as f64
                };
                return (state, fraction);
            }
        }
        if any(ItemState::Waiting) {
            return (ItemState::Waiting, 0.0);
        }
        if !files.is_empty() && files.iter().all(|item| item.state == ItemState::Downloaded) {
            return (ItemState::Downloaded, 0.0);
        }
        (ItemState::Cloud, 0.0)
    }

    /// The details of `changed`'s paths of one drive, for `ItemsChanged`.
    pub fn details_of(&self, paths: &BTreeSet<PathBuf>) -> ItemDetails {
        paths
            .iter()
            .filter_map(|path| Some((path.to_string_lossy().into_owned(), self.details(path)?)))
            .collect()
    }
}

/// What `porter-rig-storage` prints for a command it does not know.
pub const HELP: &str = "commands: offline <drive> | online <drive> | signin <drive> | pause <drive> \
     | resume <drive> | add <path> | upload <path> | wait <path> | conflict <path> \
     | error <path>   (drives: onedrive, gdrive)";

/// An online document's address in the fake.
fn online_url(path: &Path) -> String {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().replace(' ', "-"))
        .unwrap_or_default();
    format!("https://docs.example.test/document/{name}")
}

/// The file format an online document is saved as, by its link file's extension: none for a
/// Form, Site or Map.
fn export_of(document: &Path) -> Option<&'static str> {
    match document.extension()?.to_str()? {
        "gdoc" => Some("docx"),
        "gsheet" => Some("xlsx"),
        "gslides" => Some("pptx"),
        "gdraw" => Some("png"),
        _ => None,
    }
}

/// An online document's link file, as Google's desktop client writes them: a JSON object whose
/// `url` is the document's address.
fn link_file(url: &str) -> String {
    format!("{{\"url\": \"{url}\", \"doc_id\": \"fake\"}}\n")
}

/// Where the other version of the file at `path` goes: next to it, its name marked.
fn other_version(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(extension) => format!("{stem}{OTHER_VERSION}.{}", extension.to_string_lossy()),
        None => format!("{stem}{OTHER_VERSION}"),
    };
    path.with_file_name(name)
}

fn joined(parts: &[&str]) -> PathBuf {
    PathBuf::from(parts.join(" "))
}

fn put(details: &mut Details, key: &str, value: Value<'_>) {
    if let Ok(owned) = OwnedValue::try_from(value) {
        details.insert(key.to_owned(), owned);
    }
}

fn object(path: &str) -> Value<'static> {
    ObjectPath::try_from(path.to_owned()).map_or_else(|_| Value::from(path.to_owned()), Value::from)
}

/// The fake's object: the world, shared with the tick and the command reader.
#[derive(Debug, Clone)]
pub struct FakeStorage {
    world: Arc<Mutex<World>>,
}

impl FakeStorage {
    /// The object over `world`.
    pub fn new(world: World) -> FakeStorage {
        FakeStorage {
            world: Arc::new(Mutex::new(world)),
        }
    }

    /// Runs `step` on the world and sends what it changed: the drives too when `step` says so
    /// or a drive's bytes here moved.
    pub async fn apply<T>(
        &self,
        emitter: &SignalEmitter<'_>,
        step: impl FnOnce(&mut World, &mut Changed) -> (T, bool),
    ) -> T {
        let (out, signals) = {
            let mut world = self
                .world
                .lock()
                .expect("the fake's world lock is never poisoned");
            let mut changed = Changed::new();
            let before = world.locals();
            let (out, drives) = step(&mut world, &mut changed);
            let drives = drives || world.locals() != before;
            let items: Vec<(String, ItemDetails)> = changed
                .iter()
                .map(|(drive, paths)| (drive.clone(), world.details_of(paths)))
                .collect();
            (out, (items, drives))
        };
        let (items, drives) = signals;
        for (drive, details) in items {
            let _ = StorageServed::<FakeStorage>::items_changed(emitter, &drive, details).await;
        }
        if drives {
            let _ = StorageServed::<FakeStorage>::drives_changed(emitter).await;
        }
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, World> {
        self.world
            .lock()
            .expect("the fake's world lock is never poisoned")
    }
}

impl StorageServer for FakeStorage {
    type Error = fdo::Error;

    async fn drives(&self, _: Call<'_>) -> fdo::Result<Vec<(String, Details)>> {
        Ok(self.lock().drives())
    }

    async fn items(&self, _: Call<'_>, paths: Vec<String>) -> fdo::Result<ItemDetails> {
        Ok(self.lock().items(&paths))
    }

    async fn download(
        &self,
        Call { emitter, .. }: Call<'_>,
        paths: Vec<String>,
    ) -> fdo::Result<Refused> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.download(&paths, changed), false)
            })
            .await)
    }

    async fn keep(
        &self,
        Call { emitter, .. }: Call<'_>,
        paths: Vec<String>,
        keep: bool,
    ) -> fdo::Result<Refused> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.keep(&paths, keep, changed), false)
            })
            .await)
    }

    async fn remove_download(
        &self,
        Call { emitter, .. }: Call<'_>,
        paths: Vec<String>,
    ) -> fdo::Result<Refused> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.remove_download(&paths, changed), false)
            })
            .await)
    }

    async fn save_copy(
        &self,
        Call { emitter, .. }: Call<'_>,
        paths: Vec<String>,
    ) -> fdo::Result<(Saved, Refused)> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.save_copy(&paths, changed), false)
            })
            .await)
    }

    async fn set_paused(
        &self,
        Call { emitter, .. }: Call<'_>,
        drives: Vec<String>,
        paused: bool,
    ) -> fdo::Result<Refused> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.set_paused(&drives, paused, changed), true)
            })
            .await)
    }

    /// The fake tells every connection; it takes the call so Files can make it as it will to
    /// syncd.
    async fn watch(&self, _: Call<'_>) -> fdo::Result<()> {
        Ok(())
    }
}

/// Serves `storage` at `/org/quire/Storage1` on `connection` under `name`, and steps its
/// transfers every [`TICK`] until the connection closes.
pub async fn serve(connection: &Connection, name: &str, storage: FakeStorage) -> zbus::Result<()> {
    connection
        .object_server()
        .at(STORAGE_PATH, StorageServed(storage.clone()))
        .await?;
    connection.request_name(name.to_owned()).await?;
    let emitter = SignalEmitter::new(connection, STORAGE_PATH)?.into_owned();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(TICK);
        loop {
            every.tick().await;
            storage
                .apply(&emitter, |world, changed| {
                    *changed = world.tick();
                    ((), false)
                })
                .await;
        }
    });
    Ok(())
}

/// The details by key, for tests and the bin's printout.
pub fn word_of(details: &Details, key: &str) -> Option<String> {
    details
        .get(key)
        .and_then(|value| String::try_from(value.clone()).ok())
}

/// Every item path the fake plants, for printing what there is.
pub fn planted_paths(world: &World) -> Vec<PathBuf> {
    world.items.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> (test_kit::Scratch, World) {
        let dir = test_kit::Scratch::new("storage-fake");
        let world = World::plant(dir.path()).expect("the fake plants its drives");
        (dir, world)
    }

    fn path(dir: &test_kit::Scratch, under: &str) -> String {
        dir.path()
            .join("drives")
            .join(under)
            .to_string_lossy()
            .into_owned()
    }

    fn state(world: &World, at: &str) -> String {
        let items = world.items(&[at.to_owned()]);
        word_of(&items[at], ITEM_KEY_STATE).expect("every item has a state")
    }

    fn kept(world: &World, at: &str) -> bool {
        let items = world.items(&[at.to_owned()]);
        bool::try_from(items[at][ITEM_KEY_KEPT].clone()).expect("every item says whether kept")
    }

    fn fraction(world: &World, at: &str) -> Option<f64> {
        let items = world.items(&[at.to_owned()]);
        items[at]
            .get(ITEM_KEY_FRACTION)
            .and_then(|value| f64::try_from(value.clone()).ok())
    }

    fn local(world: &World, drive: &str) -> u64 {
        let (_, details) = world
            .drives()
            .into_iter()
            .find(|(id, _)| id == drive)
            .expect("the fake has this drive");
        u64::try_from(details[DRIVE_KEY_LOCAL].clone()).expect("every drive says its bytes here")
    }

    #[test]
    fn a_download_fills_its_fraction_and_ends_downloaded() {
        let (dir, mut world) = world();
        let beach = path(&dir, "OneDrive/Pictures/Beach.jpg");
        let mut changed = Changed::new();
        assert!(
            world
                .download(std::slice::from_ref(&beach), &mut changed)
                .is_empty()
        );
        assert_eq!(state(&world, &beach), "downloading");
        let mut ticks = 0;
        while state(&world, &beach) == "downloading" {
            let moved = world.tick();
            assert!(
                moved["onedrive"].contains(Path::new(&beach)),
                "a moving file is told each tick"
            );
            ticks += 1;
            assert!(ticks <= 20, "a download ends in twenty ticks");
        }
        assert_eq!(state(&world, &beach), "downloaded");
        assert_eq!(
            state(&world, &path(&dir, "OneDrive/Pictures")),
            "waiting",
            "Garden still waits"
        );
    }

    #[test]
    fn remove_download_refuses_what_is_waiting_and_frees_the_rest() {
        let (dir, mut world) = world();
        let garden = path(&dir, "OneDrive/Pictures/Garden.jpg");
        let notes = path(&dir, "OneDrive/Documents/Notes.txt");
        let refused = world.remove_download(&[garden.clone(), notes.clone()], &mut Changed::new());
        assert_eq!(
            refused.get(&garden).map(String::as_str),
            Some(words::WAITING)
        );
        assert!(!refused.contains_key(&notes));
        assert_eq!(state(&world, &notes), "cloud");
    }

    #[test]
    fn an_online_document_is_never_downloaded_and_offline_waits() {
        let (dir, mut world) = world();
        let plan = path(&dir, "Google Drive/Projects/Plan.gdoc");
        let refused = world.download(std::slice::from_ref(&plan), &mut Changed::new());
        assert_eq!(
            refused.get(&plan).map(String::as_str),
            Some(words::ONLINE_ONLY)
        );
        world.command("offline gdrive").expect("a known command");
        let sketch = path(&dir, "Google Drive/Projects/Sketch.png");
        let refused = world.download(std::slice::from_ref(&sketch), &mut Changed::new());
        assert_eq!(
            refused.get(&sketch).map(String::as_str),
            Some(words::OFFLINE)
        );
    }

    #[test]
    fn a_folder_carries_the_fraction_of_what_moves_under_it() {
        let (dir, mut world) = world();
        let report = path(&dir, "OneDrive/Documents/Report.docx");
        let documents = path(&dir, "OneDrive/Documents");
        world.download(std::slice::from_ref(&report), &mut Changed::new());
        for _ in 0..4 {
            world.tick();
        }
        assert_eq!(state(&world, &documents), "downloading");
        let (folder, file) = (fraction(&world, &documents), fraction(&world, &report));
        let (Some(folder), Some(file)) = (folder, file) else {
            panic!("a moving folder and file both carry a fraction");
        };
        assert!(
            (folder - file).abs() < 1e-9,
            "the one file moving is the folder's share"
        );
        assert!(folder > 0.0 && folder < 1.0);
    }

    #[test]
    fn a_kept_folder_keeps_what_is_added_to_it_later() {
        let (dir, mut world) = world();
        let pictures = path(&dir, "OneDrive/Pictures");
        let beach = path(&dir, "OneDrive/Pictures/Beach.jpg");
        world.keep(std::slice::from_ref(&pictures), true, &mut Changed::new());
        assert!(kept(&world, &beach), "a child of a kept folder reads kept");
        let added = path(&dir, "OneDrive/Pictures/Hike.jpg");
        world
            .command(&format!("add {added}"))
            .expect("a known command");
        assert!(kept(&world, &added));
        assert_eq!(state(&world, &added), "downloading");
        let refused = world.keep(std::slice::from_ref(&beach), false, &mut Changed::new());
        assert_eq!(
            refused.get(&beach).map(String::as_str),
            Some(words::KEPT_WITH_FOLDER)
        );
        world.keep(std::slice::from_ref(&pictures), false, &mut Changed::new());
        assert!(!kept(&world, &beach) && !kept(&world, &added));
    }

    #[test]
    fn an_online_document_gives_its_address_and_a_conflict_names_the_other_version() {
        let (dir, world) = world();
        let plan = path(&dir, "Google Drive/Projects/Plan.gdoc");
        let items = world.items(std::slice::from_ref(&plan));
        let url = word_of(&items[&plan], ITEM_KEY_URL).expect("an online document has a url");
        let link = std::fs::read_to_string(&plan).expect("the link file is on disk");
        assert!(link.contains(&format!("\"url\": \"{url}\"")));

        let budget = path(&dir, "OneDrive/Budget.xlsx");
        let items = world.items(std::slice::from_ref(&budget));
        let other = word_of(&items[&budget], ITEM_KEY_OTHER).expect("a conflict names the other");
        assert!(Path::new(&other).is_file());
        let back = world.items(std::slice::from_ref(&other));
        assert_eq!(word_of(&back[&other], ITEM_KEY_OTHER), Some(budget));
    }

    #[test]
    fn pausing_holds_uploads_and_removing_a_download_frees_its_bytes() {
        let (dir, mut world) = world();
        let garden = path(&dir, "OneDrive/Pictures/Garden.jpg");
        world.set_paused(&["onedrive".into()], false, &mut Changed::new());
        assert_eq!(state(&world, &garden), "uploading");
        world.set_paused(&["onedrive".into()], true, &mut Changed::new());
        assert_eq!(state(&world, &garden), "waiting");
        let refused = world.set_paused(&["nowhere".into()], true, &mut Changed::new());
        assert_eq!(
            refused.get("nowhere").map(String::as_str),
            Some(words::NO_DRIVE)
        );

        let notes = path(&dir, "OneDrive/Documents/Notes.txt");
        let before = local(&world, "onedrive");
        world.remove_download(std::slice::from_ref(&notes), &mut Changed::new());
        assert_eq!(local(&world, "onedrive"), before - 4_000);
    }

    #[test]
    fn save_a_copy_names_each_copy_as_a_mac_does_and_lists_it_downloaded() {
        let (dir, mut world) = world();
        let plan = path(&dir, "Google Drive/Projects/Plan.gdoc");
        let mut names = Vec::new();
        for _ in 0..3 {
            let mut changed = Changed::new();
            let (saved, refused) = world.save_copy(std::slice::from_ref(&plan), &mut changed);
            assert!(refused.is_empty(), "{refused:?}");
            let copy = saved[&plan].clone();
            assert!(
                changed["gdrive"].contains(Path::new(&copy)),
                "the copy is told"
            );
            assert_eq!(state(&world, &copy), "downloaded");
            assert!(
                Path::new(&copy).is_file(),
                "the copy is a file on the drive"
            );
            names.push(
                Path::new(&copy)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        assert_eq!(names, ["Plan.docx", "Plan 2.docx", "Plan 3.docx"]);
    }

    #[test]
    fn save_a_copy_refuses_a_file_a_form_and_a_path_on_no_drive() {
        let (dir, mut world) = world();
        let sketch = path(&dir, "Google Drive/Projects/Sketch.png");
        let survey = path(&dir, "Google Drive/Projects/Survey.gform");
        let nowhere = dir
            .path()
            .join("elsewhere.gdoc")
            .to_string_lossy()
            .into_owned();
        let (saved, refused) = world.save_copy(
            &[sketch.clone(), survey.clone(), nowhere.clone()],
            &mut Changed::new(),
        );
        assert!(saved.is_empty());
        assert_eq!(
            refused.get(&sketch).map(String::as_str),
            Some(words::NOT_A_DOCUMENT)
        );
        assert_eq!(
            refused.get(&survey).map(String::as_str),
            Some(words::NO_FILE_FORMAT)
        );
        assert_eq!(
            refused.get(&nowhere).map(String::as_str),
            Some(words::NOT_ON_A_DRIVE)
        );
    }

    #[test]
    fn keep_downloaded_marks_the_folder_itself_kept() {
        let (dir, mut world) = world();
        let pictures = path(&dir, "OneDrive/Pictures");
        world.keep(std::slice::from_ref(&pictures), true, &mut Changed::new());
        let items = world.items(std::slice::from_ref(&pictures));
        assert_eq!(
            bool::try_from(items[&pictures][ITEM_KEY_KEPT].clone()).ok(),
            Some(true)
        );
    }
}
