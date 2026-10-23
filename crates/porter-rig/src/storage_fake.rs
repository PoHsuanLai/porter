//! A fake `org.quire.Storage1` for building Files before syncd serves the real one: two drives
//! of real files under a scratch folder, whose states move as the real ones will. Download Now
//! fills a progress fraction ten times a second until the file is downloaded; Keep Downloaded and
//! Remove Download change what they say; the commands of [`World::command`] (read from stdin by
//! `porter-rig-storage`) take a drive offline, ask for sign-in again, or start an upload, an
//! error or a conflict on one item.
//!
//! The files on disk are ordinary files with a line of text: nothing here is downloaded from
//! anywhere, only the states are. Test tooling, never installed.

use porter_dbus::{
    DRIVE_KEY_ACCOUNT, DRIVE_KEY_LABEL, DRIVE_KEY_PATH, DRIVE_KEY_PROVIDER, DRIVE_KEY_STATE,
    DRIVE_KEY_TOTAL, DRIVE_KEY_USED, DRIVE_KEY_WORDS, Details, DriveState, ITEM_KEY_FRACTION,
    ITEM_KEY_KEPT, ITEM_KEY_KIND, ITEM_KEY_STATE, ITEM_KEY_WORDS, ItemDetails, ItemKind, ItemState,
    Refused, STORAGE_PATH,
};
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

/// The words a refused action gives, as Files may show them.
pub mod words {
    /// A path on no drive.
    pub const NOT_ON_A_DRIVE: &str = "This item isn't in a cloud drive.";
    /// An online document asked to download, keep or remove.
    pub const ONLINE_ONLY: &str = "This document opens online.";
    /// A download asked for while the drive is offline.
    pub const OFFLINE: &str = "This file will download when you're connected.";
    /// A removal asked for an item with changes not sent yet.
    pub const WAITING: &str = "Waiting to upload.";
    /// A removal asked for an item with two versions.
    pub const CONFLICT: &str = "This file has two versions. Choose one first.";
}

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

/// One file or folder of the fake. A folder's state is worked out from what is under it.
#[derive(Debug, Clone)]
struct Item {
    drive: String,
    kind: ItemKind,
    state: ItemState,
    kept: bool,
    fraction: f64,
    words: Option<String>,
}

/// What a step changed: per drive, the paths whose details are new.
pub type Changed = BTreeMap<String, BTreeSet<PathBuf>>;

/// The fake's drives and items.
#[derive(Debug)]
pub struct World {
    drives: Vec<Drive>,
    items: BTreeMap<PathBuf, Item>,
}

/// Each file the fake plants: its drive, its path under the drive, its kind and its state.
const PLANTED: &[(&str, &str, ItemKind, ItemState, bool)] = &[
    (
        "onedrive",
        "Documents",
        ItemKind::Folder,
        ItemState::Cloud,
        false,
    ),
    (
        "onedrive",
        "Documents/Report.docx",
        ItemKind::File,
        ItemState::Cloud,
        false,
    ),
    (
        "onedrive",
        "Documents/Notes.txt",
        ItemKind::File,
        ItemState::Downloaded,
        false,
    ),
    (
        "onedrive",
        "Documents/Taxes 2025.pdf",
        ItemKind::File,
        ItemState::Downloaded,
        true,
    ),
    (
        "onedrive",
        "Pictures",
        ItemKind::Folder,
        ItemState::Cloud,
        false,
    ),
    (
        "onedrive",
        "Pictures/Beach.jpg",
        ItemKind::File,
        ItemState::Cloud,
        false,
    ),
    (
        "onedrive",
        "Pictures/Garden.jpg",
        ItemKind::File,
        ItemState::Waiting,
        false,
    ),
    (
        "onedrive",
        "Budget.xlsx",
        ItemKind::File,
        ItemState::Conflict,
        false,
    ),
    (
        "gdrive",
        "Projects",
        ItemKind::Folder,
        ItemState::Cloud,
        false,
    ),
    (
        "gdrive",
        "Projects/Plan.gdoc",
        ItemKind::Online,
        ItemState::Cloud,
        false,
    ),
    (
        "gdrive",
        "Projects/Sketch.png",
        ItemKind::File,
        ItemState::Cloud,
        false,
    ),
    (
        "gdrive",
        "Projects/Old draft.odt",
        ItemKind::File,
        ItemState::Error,
        false,
    ),
    (
        "gdrive",
        "Recipes.pdf",
        ItemKind::File,
        ItemState::Downloaded,
        false,
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
        let mut items = BTreeMap::new();
        for drive in &drives {
            std::fs::create_dir_all(&drive.path)?;
        }
        for (id, under, kind, state, kept) in PLANTED {
            let drive = drives
                .iter()
                .find(|drive| drive.id == *id)
                .expect("a planted item's drive is one of the two");
            let path = drive.path.join(under);
            match kind {
                ItemKind::Folder => std::fs::create_dir_all(&path)?,
                ItemKind::Online => std::fs::write(
                    &path,
                    "{\"url\": \"https://docs.example.test/fake\", \"note\": \"a fake online document\"}\n",
                )?,
                _ => std::fs::write(&path, format!("A fake file: {under}\n"))?,
            }
            let words =
                (*state == ItemState::Error).then(|| "The server could not send it.".into());
            items.insert(
                path,
                Item {
                    drive: (*id).into(),
                    kind: *kind,
                    state: *state,
                    kept: *kept,
                    fraction: 0.0,
                    words,
                },
            );
        }
        Ok(World { drives, items })
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

    /// "Keep Downloaded" on or off.
    pub fn keep(&mut self, paths: &[String], keep: bool, changed: &mut Changed) -> Refused {
        self.each_file(paths, changed, |world, path, changed| {
            if let Some(item) = world.items.get_mut(path) {
                item.kept = keep;
            }
            world.touch(path, changed);
            if keep {
                world.start_download(path, changed)
            } else {
                None
            }
        })
    }

    /// "Remove Download".
    pub fn remove_download(&mut self, paths: &[String], changed: &mut Changed) -> Refused {
        self.each_file(paths, changed, |world, path, changed| {
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
        })
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
            ("online" | "ready", [drive]) => {
                let back = self.set_drive(drive, DriveState::Ready, None)?;
                // Back online: what waited is sent.
                let waiting: Vec<PathBuf> = self
                    .items
                    .iter()
                    .filter(|(_, item)| item.drive == *drive && item.state == ItemState::Waiting)
                    .map(|(path, _)| path.clone())
                    .collect();
                for path in waiting {
                    self.set_item(&path, ItemState::Uploading, None, &mut changed)?;
                }
                back
            }
            ("signin", [drive]) => self.set_drive(
                drive,
                DriveState::NeedsSignIn,
                Some("Sign in again to see this drive's files.".into()),
            )?,
            ("pause", [drive]) => self.set_drive(drive, DriveState::Paused, None)?,
            ("upload", path) if !path.is_empty() => {
                self.set_item(&joined(path), ItemState::Uploading, None, &mut changed)?;
                false
            }
            ("wait", path) if !path.is_empty() => {
                self.set_item(&joined(path), ItemState::Waiting, None, &mut changed)?;
                false
            }
            ("conflict", path) if !path.is_empty() => {
                self.set_item(&joined(path), ItemState::Conflict, None, &mut changed)?;
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

    /// Runs `act` on every file of `paths` (a folder: every file under it), gathering refusals by
    /// the path the caller gave.
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
            match item.kind {
                ItemKind::Online => {
                    refused.insert(text.clone(), words::ONLINE_ONLY.into());
                }
                ItemKind::Folder => {
                    let under: Vec<PathBuf> = self
                        .items
                        .iter()
                        .filter(|(inner, item)| {
                            inner.starts_with(&path) && item.kind == ItemKind::File
                        })
                        .map(|(inner, _)| inner.clone())
                        .collect();
                    if let Some(why) = under.iter().find_map(|inner| act(self, inner, changed)) {
                        refused.insert(text.clone(), why);
                    }
                }
                _ => {
                    if let Some(why) = act(self, &path, changed) {
                        refused.insert(text.clone(), why);
                    }
                }
            }
        }
        refused
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
        let kept = match item.kind {
            ItemKind::Folder => {
                let mut files = self.files_under(path).peekable();
                files.peek().is_some() && files.all(|inner| inner.kept)
            }
            _ => item.kept,
        };
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
        Some(details)
    }

    fn files_under<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Item> + 'a {
        self.items
            .iter()
            .filter(move |(inner, item)| inner.starts_with(path) && item.kind == ItemKind::File)
            .map(|(_, item)| item)
    }

    /// A folder's state: conflict, error, moving (with the mean fraction of what moves),
    /// downloaded when every file is, else cloud.
    fn folder_state(&self, path: &Path) -> (ItemState, f64) {
        let files: Vec<&Item> = self.files_under(path).collect();
        let any = |state: ItemState| files.iter().any(|item| item.state == state);
        for state in [ItemState::Conflict, ItemState::Error] {
            if any(state) {
                return (state, 0.0);
            }
        }
        for state in [ItemState::Uploading, ItemState::Downloading] {
            let moving: Vec<f64> = files
                .iter()
                .filter(|item| item.state == state)
                .map(|item| item.fraction)
                .collect();
            if !moving.is_empty() {
                return (state, moving.iter().sum::<f64>() / moving.len() as f64);
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
     | upload <path> | wait <path> | conflict <path> | error <path>   (drives: onedrive, gdrive)";

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

    /// Runs `step` on the world and sends what it changed.
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
            let (out, drives) = step(&mut world, &mut changed);
            let items: Vec<(String, ItemDetails)> = changed
                .iter()
                .map(|(drive, paths)| (drive.clone(), world.details_of(paths)))
                .collect();
            (out, (items, drives))
        };
        let (items, drives) = signals;
        for (drive, details) in items {
            let _ = FakeStorage::items_changed(emitter, &drive, details).await;
        }
        if drives {
            let _ = FakeStorage::drives_changed(emitter).await;
        }
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, World> {
        self.world
            .lock()
            .expect("the fake's world lock is never poisoned")
    }
}

#[zbus::interface(name = "org.quire.Storage1")]
impl FakeStorage {
    fn drives(&self) -> fdo::Result<Vec<(String, Details)>> {
        Ok(self.lock().drives())
    }

    fn items(&self, paths: Vec<String>) -> fdo::Result<ItemDetails> {
        Ok(self.lock().items(&paths))
    }

    async fn download(
        &self,
        paths: Vec<String>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<Refused> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.download(&paths, changed), false)
            })
            .await)
    }

    async fn keep(
        &self,
        paths: Vec<String>,
        keep: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<Refused> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.keep(&paths, keep, changed), false)
            })
            .await)
    }

    async fn remove_download(
        &self,
        paths: Vec<String>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<Refused> {
        Ok(self
            .apply(&emitter, |world, changed| {
                (world.remove_download(&paths, changed), false)
            })
            .await)
    }

    /// The fake tells every connection; it takes the call so Files can make it as it will to
    /// syncd.
    fn watch(&self) -> fdo::Result<()> {
        Ok(())
    }

    #[zbus(signal)]
    async fn drives_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn items_changed(
        emitter: &SignalEmitter<'_>,
        drive: &str,
        items: ItemDetails,
    ) -> zbus::Result<()>;
}

/// Serves `storage` at `/org/quire/Storage1` on `connection` under `name`, and steps its
/// transfers every [`TICK`] until the connection closes.
pub async fn serve(connection: &Connection, name: &str, storage: FakeStorage) -> zbus::Result<()> {
    connection
        .object_server()
        .at(STORAGE_PATH, storage.clone())
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

    fn world() -> (porter_fake::Scratch, World) {
        let dir = porter_fake::Scratch::new("storage-fake");
        let world = World::plant(dir.path()).expect("the fake plants its drives");
        (dir, world)
    }

    fn path(dir: &porter_fake::Scratch, under: &str) -> String {
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

    #[test]
    fn a_download_fills_its_fraction_and_ends_downloaded() {
        let (dir, mut world) = world();
        let beach = path(&dir, "OneDrive/Pictures/Beach.jpg");
        let mut changed = Changed::new();
        assert!(world.download(&[beach.clone()], &mut changed).is_empty());
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
        let refused = world.download(&[plan.clone()], &mut Changed::new());
        assert_eq!(
            refused.get(&plan).map(String::as_str),
            Some(words::ONLINE_ONLY)
        );
        world.command("offline gdrive").expect("a known command");
        let sketch = path(&dir, "Google Drive/Projects/Sketch.png");
        let refused = world.download(&[sketch.clone()], &mut Changed::new());
        assert_eq!(
            refused.get(&sketch).map(String::as_str),
            Some(words::OFFLINE)
        );
    }

    #[test]
    fn keep_downloaded_marks_a_folder_kept_once_every_file_in_it_is() {
        let (dir, mut world) = world();
        let pictures = path(&dir, "OneDrive/Pictures");
        world.keep(&[pictures.clone()], true, &mut Changed::new());
        let items = world.items(&[pictures.clone()]);
        assert_eq!(
            bool::try_from(items[&pictures][ITEM_KEY_KEPT].clone()).ok(),
            Some(true)
        );
    }
}
