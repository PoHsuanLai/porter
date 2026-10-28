//! `org.quire.Storage1` at `/org/quire/Storage1` on syncd's connection: the drives shown in
//! Files and their items, served from each drive's files on demand ([`DriveSet`]).
//!
//! - The caller is the connection's, found by porter-dbus's `Callers` as for `Sync1`. The shell
//!   (`CallerRole::SheetHost`) and Settings (`CallerRole::Settings`) see every drive; an app sees
//!   them when the person let it see their files (the apps of [`FilesApps`]); anyone else, and a
//!   sender syncd does not know, is `AccessDenied`, for every method. `SetPaused` is the shell's
//!   and Settings' only: an app is `AccessDenied` there too.
//! - A path is absolute and inside one drive's `path`, by the names the drive's items are shown
//!   by (`porter_drive::naming`). A path on no drive, or not there, is left out of `Items` and
//!   refused by the actions with `NOT_ON_A_DRIVE`; the drive's own folder is no item, and
//!   neither is what stays on this computer (`porter_drive::local_only`: the desktop's trash).
//! - An item changed here is `waiting`, then `uploading` with its fraction while it is sent; a
//!   file kept in two versions is `conflict`, and `other` is the other version's path; one the
//!   drive refused is `error` with the words (`crate::words`).
//! - `SaveCopy` exports each online document (`porter_drive::Drive::save_copy`) and answers each
//!   copy's absolute path; the copy waits to upload beside its document.
//! - `Watch` joins the caller's connection to those told `DrivesChanged` and `ItemsChanged`
//!   until it leaves the bus. Each drive's item changes are gathered and sent together at most
//!   ten times a second, each changed item with the folders above it.

use super::errors::RefusedError;
use crate::datasets::drive::{DriveEvent, DriveSet, LiveDrive};
use crate::service::Access;
use crate::words;
use porter_callers::{Caller, CallerRole, Callers};
use porter_dbus::Call;
use porter_dbus::Details;
use porter_dbus::sync::STORAGE_PATH;
use porter_dbus::sync::{
    DRIVE_KEY_ACCOUNT, DRIVE_KEY_LABEL, DRIVE_KEY_LOCAL, DRIVE_KEY_PATH, DRIVE_KEY_PROVIDER,
    DRIVE_KEY_STATE, DRIVE_KEY_TOTAL, DRIVE_KEY_USED, DRIVE_KEY_WORDS, ITEM_KEY_FRACTION,
    ITEM_KEY_KEPT, ITEM_KEY_KIND, ITEM_KEY_OTHER, ITEM_KEY_STATE, ITEM_KEY_URL, ITEM_KEY_WORDS,
    ItemDetails, ItemKind, ItemState, Refused, Saved, StorageServed, StorageServer,
};
use porter_drive::{Node, NodeId, NodeKind, Reason, Status};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use zbus::Connection;
use zbus::export::futures_core::Stream;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

/// How often a drive's item changes are sent at most.
pub const ITEMS_EVERY: Duration = Duration::from_millis(100);

/// The apps the person let see their files: they see every drive. (The shell and Settings
/// always do.)
pub type FilesApps = Access;

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Core<C> {
    drives: DriveSet,
    callers: C,
    apps: FilesApps,
    connection: Connection,
    /// The connections that called `Watch`, by unique name.
    watchers: Mutex<BTreeMap<String, Caller>>,
}

/// Whether `caller` may use the interface at all.
fn admitted(apps: &FilesApps, caller: &Caller) -> bool {
    match caller.role {
        CallerRole::SheetHost | CallerRole::Settings => true,
        CallerRole::App => apps.owners.contains(&caller.app.name),
        _ => false,
    }
}

fn put(details: &mut Details, key: &str, value: Value<'_>) {
    if let Ok(owned) = OwnedValue::try_from(value) {
        details.insert(key.to_owned(), owned);
    }
}

fn state_of(status: &Status) -> ItemState {
    match status.state {
        porter_drive::ItemState::Downloading => ItemState::Downloading,
        porter_drive::ItemState::Downloaded => ItemState::Downloaded,
        porter_drive::ItemState::Waiting => ItemState::Waiting,
        porter_drive::ItemState::Uploading => ItemState::Uploading,
        porter_drive::ItemState::Conflict => ItemState::Conflict,
        porter_drive::ItemState::Error => ItemState::Error,
        _ => ItemState::Cloud,
    }
}

/// An item's details as `Items` gives them, on `drive` (where its other version is).
pub fn item_details(drive: &LiveDrive, status: &Status) -> Details {
    let mut details = Details::new();
    let state = state_of(status);
    put(&mut details, ITEM_KEY_STATE, Value::from(state.as_str()));
    put(&mut details, ITEM_KEY_KEPT, Value::Bool(status.kept));
    let kind = match &status.kind {
        NodeKind::Folder => ItemKind::Folder,
        NodeKind::File => ItemKind::File,
        NodeKind::Online { .. } => ItemKind::Online,
    };
    put(&mut details, ITEM_KEY_KIND, Value::from(kind.as_str()));
    if let (ItemState::Downloading | ItemState::Uploading, Some(fraction)) =
        (state, status.fraction)
    {
        put(&mut details, ITEM_KEY_FRACTION, Value::F64(fraction));
    }
    if let (ItemState::Conflict, Some(other)) = (state, &status.other)
        && let Some(path) = absolute(drive, other)
    {
        put(
            &mut details,
            ITEM_KEY_OTHER,
            Value::from(path.to_string_lossy().as_ref()),
        );
    }
    if let (ItemState::Error, Some(reason)) = (state, status.reason) {
        put(
            &mut details,
            ITEM_KEY_WORDS,
            Value::from(words::reason(reason)),
        );
    }
    if let NodeKind::Online { url, .. } = &status.kind {
        put(&mut details, ITEM_KEY_URL, Value::from(url.as_str()));
    }
    details
}

/// A drive's row as `Drives` gives it.
pub fn drive_details(drive: &LiveDrive) -> Details {
    let mut details = Details::new();
    let account = ObjectPath::try_from(drive.account.clone())
        .map_or_else(|_| Value::from(drive.account.clone()), Value::from);
    put(&mut details, DRIVE_KEY_ACCOUNT, account);
    put(
        &mut details,
        DRIVE_KEY_LABEL,
        Value::from(drive.label.as_str()),
    );
    put(
        &mut details,
        DRIVE_KEY_PROVIDER,
        Value::from(drive.provider),
    );
    put(
        &mut details,
        DRIVE_KEY_PATH,
        Value::from(drive.path.to_string_lossy().as_ref()),
    );
    let (state, words) = drive.state();
    put(&mut details, DRIVE_KEY_STATE, Value::from(state.as_str()));
    put(
        &mut details,
        DRIVE_KEY_LOCAL,
        Value::U64(drive.drive.local_bytes()),
    );
    if let Some((used, total)) = drive.drive.quota() {
        put(&mut details, DRIVE_KEY_USED, Value::U64(used));
        if let Some(total) = total {
            put(&mut details, DRIVE_KEY_TOTAL, Value::U64(total));
        }
    }
    if let Some(words) = words {
        put(&mut details, DRIVE_KEY_WORDS, Value::from(words));
    }
    details
}

impl<C: Callers> Core<C> {
    /// Who the sender is, if it may use the interface.
    async fn identify(&self, header: &Header<'_>) -> Result<Caller, RefusedError> {
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        let caller = self
            .callers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(|| RefusedError::access_denied("syncd does not know this caller"))?;
        match admitted(&self.apps, &caller) {
            true => Ok(caller),
            false => Err(RefusedError::access_denied(
                "this caller may not see the person's files",
            )),
        }
    }

    /// The drive and node at `path`.
    fn find(&self, path: &str) -> Option<(Arc<LiveDrive>, Node)> {
        let path = Path::new(path);
        self.drives.all().into_iter().find_map(|drive| {
            let inside = path.strip_prefix(&drive.path).ok()?;
            if inside.as_os_str().is_empty() {
                return None;
            }
            let node = drive.drive.resolve(inside).ok().flatten()?;
            if drive.drive.local_only(&node.id) {
                return None;
            }
            Some((drive, node))
        })
    }

    /// Runs `act` on each of `paths` found on a drive; what it refused, and every path on no
    /// drive.
    fn each(
        &self,
        paths: &[String],
        mut act: impl FnMut(&LiveDrive, &NodeId) -> Result<(), Reason>,
    ) -> Refused {
        let mut refused = Refused::new();
        for text in paths {
            match self.find(text) {
                Some((drive, node)) => {
                    if let Err(why) = act(&drive, &node.id) {
                        refused.insert(text.clone(), words::reason(why).to_owned());
                    }
                }
                None => {
                    refused.insert(text.clone(), words::reason(Reason::NotOnADrive).to_owned());
                }
            }
        }
        refused
    }

    /// `id`'s absolute path and its details, then each folder above it, on `drive`.
    fn with_folders(&self, drive: &LiveDrive, id: &NodeId, into: &mut ItemDetails) {
        if drive.drive.local_only(id) {
            return;
        }
        let Ok(Some(path)) = drive.drive.path_of(id) else {
            return;
        };
        let mut at: Option<&Path> = Some(&path);
        let mut ids = vec![id.clone()];
        // The folders above, by their ids.
        let mut up = id.clone();
        while let Ok(Some(node)) = drive.drive.node(&up) {
            if node.parent == NodeId::Top {
                break;
            }
            ids.push(node.parent.clone());
            up = node.parent;
        }
        for id in ids {
            let Some(here) = at else { break };
            let key = drive.path.join(here).to_string_lossy().into_owned();
            if !into.contains_key(&key)
                && let Some(status) = drive.drive.status(&id)
            {
                into.insert(key, item_details(drive, &status));
            }
            at = here.parent().filter(|p| !p.as_os_str().is_empty());
        }
    }

    async fn tell_items(&self, drive: &LiveDrive, ids: &BTreeSet<NodeId>) {
        let mut details = ItemDetails::new();
        for id in ids {
            self.with_folders(drive, id, &mut details);
        }
        if details.is_empty() {
            return;
        }
        for name in self.watcher_names() {
            if let Ok(emitter) = self.emitter(&name) {
                let _ = StorageServed::<StorageObject<C>>::items_changed(
                    &emitter,
                    &drive.id,
                    details.clone(),
                )
                .await;
            }
        }
    }

    async fn tell_drives(&self) {
        for name in self.watcher_names() {
            if let Ok(emitter) = self.emitter(&name) {
                let _ = StorageServed::<StorageObject<C>>::drives_changed(&emitter).await;
            }
        }
    }

    fn watcher_names(&self) -> Vec<String> {
        held(&self.watchers).keys().cloned().collect()
    }

    fn emitter(&self, name: &str) -> zbus::Result<SignalEmitter<'static>> {
        Ok(SignalEmitter::new(&self.connection, STORAGE_PATH)?
            .set_destination(BusName::try_from(name.to_owned())?)
            .into_owned())
    }
}

/// The interface object.
struct StorageObject<C>(Arc<Core<C>>);

impl<C: Callers> StorageServer for StorageObject<C> {
    type Error = RefusedError;

    async fn drives(
        &self,
        Call { header, .. }: Call<'_>,
    ) -> Result<Vec<(String, Details)>, RefusedError> {
        self.0.identify(&header).await?;
        Ok(self
            .0
            .drives
            .all()
            .iter()
            .map(|drive| (drive.id.clone(), drive_details(drive)))
            .collect())
    }

    async fn items(
        &self,
        Call { header, .. }: Call<'_>,
        paths: Vec<String>,
    ) -> Result<ItemDetails, RefusedError> {
        self.0.identify(&header).await?;
        Ok(paths
            .into_iter()
            .filter_map(|text| {
                let (drive, node) = self.0.find(&text)?;
                let status = drive.drive.status(&node.id)?;
                Some((text, item_details(&drive, &status)))
            })
            .collect())
    }

    async fn download(
        &self,
        Call { header, .. }: Call<'_>,
        paths: Vec<String>,
    ) -> Result<Refused, RefusedError> {
        self.0.identify(&header).await?;
        Ok(self.0.each(&paths, |drive, id| drive.drive.download(id)))
    }

    async fn keep(
        &self,
        Call { header, .. }: Call<'_>,
        paths: Vec<String>,
        keep: bool,
    ) -> Result<Refused, RefusedError> {
        self.0.identify(&header).await?;
        Ok(self.0.each(&paths, |drive, id| drive.drive.keep(id, keep)))
    }

    async fn remove_download(
        &self,
        Call { header, .. }: Call<'_>,
        paths: Vec<String>,
    ) -> Result<Refused, RefusedError> {
        self.0.identify(&header).await?;
        Ok(self
            .0
            .each(&paths, |drive, id| drive.drive.remove_download(id)))
    }

    async fn save_copy(
        &self,
        Call { header, .. }: Call<'_>,
        paths: Vec<String>,
    ) -> Result<(Saved, Refused), RefusedError> {
        self.0.identify(&header).await?;
        let mut saved = Saved::new();
        let mut refused = Refused::new();
        for text in paths {
            let Some((drive, node)) = self.0.find(&text) else {
                refused.insert(text, words::reason(Reason::NotOnADrive).to_owned());
                continue;
            };
            match drive.drive.save_copy(&node.id).await {
                Ok(copy) => match absolute(&drive, &copy) {
                    Some(path) => {
                        saved.insert(text, path.to_string_lossy().into_owned());
                    }
                    None => {
                        refused.insert(text, words::reason(Reason::CopyFailed).to_owned());
                    }
                },
                Err(why) => {
                    refused.insert(text, words::reason(why).to_owned());
                }
            }
        }
        Ok((saved, refused))
    }

    async fn set_paused(
        &self,
        Call { header, .. }: Call<'_>,
        drives: Vec<String>,
        paused: bool,
    ) -> Result<Refused, RefusedError> {
        let caller = self.0.identify(&header).await?;
        if !matches!(caller.role, CallerRole::SheetHost | CallerRole::Settings) {
            return Err(RefusedError::access_denied(
                "only the shell and Settings pause a drive",
            ));
        }
        let mut refused = Refused::new();
        for id in drives {
            // Kept across restarts.
            if !self.0.drives.set_paused(&id, paused) {
                refused.insert(id, words::reason(Reason::NoDrive).to_owned());
            }
        }
        self.0.drives.tell_drives();
        Ok(refused)
    }

    async fn watch(&self, Call { header, .. }: Call<'_>) -> Result<(), RefusedError> {
        let caller = self.0.identify(&header).await?;
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        held(&self.0.watchers).insert(sender.to_string(), caller);
        // A sender that left before it was kept may have had its going heard first: asked
        // after, the bus says it has no owner, and it is not kept.
        let gone = match DBusProxy::new(&self.0.connection).await {
            Ok(bus) => matches!(
                bus.get_name_owner(BusName::Unique(sender.to_owned())).await,
                Err(zbus::fdo::Error::NameHasNoOwner(_))
            ),
            Err(_) => false,
        };
        if gone {
            held(&self.0.watchers).remove(sender.as_str());
        }
        Ok(())
    }
}

/// Serves `org.quire.Storage1` at [`STORAGE_PATH`] on `connection` over `drives`, with
/// `callers` saying who calls and `apps` the apps that may see the person's files. Call it
/// before [`super::serve`], which claims the name once every object is there.
pub async fn serve_storage<C: Callers>(
    connection: &Connection,
    drives: DriveSet,
    callers: C,
    apps: FilesApps,
) -> zbus::Result<()> {
    let core = Arc::new(Core {
        drives,
        callers,
        apps,
        connection: connection.clone(),
        watchers: Mutex::new(BTreeMap::new()),
    });
    connection
        .object_server()
        .at(
            STORAGE_PATH,
            StorageServed(StorageObject(Arc::clone(&core))),
        )
        .await?;
    let mut owners = DBusProxy::new(connection)
        .await?
        .receive_name_owner_changed()
        .await?;
    let weak = Arc::downgrade(&core);
    tokio::spawn(async move {
        while let Some(signal) =
            std::future::poll_fn(|cx| Pin::new(&mut owners).poll_next(cx)).await
        {
            let Some(core) = weak.upgrade() else { return };
            if let Ok(args) = signal.args()
                && args.new_owner().is_none()
            {
                held(&core.watchers).remove(args.name().as_str());
            }
        }
    });
    let mut events = core.drives.subscribe();
    let weak = Arc::downgrade(&core);
    tokio::spawn(async move {
        let mut items: BTreeMap<String, BTreeSet<NodeId>> = BTreeMap::new();
        let mut drives = false;
        let mut every = tokio::time::interval(ITEMS_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = events.recv() => {
                    let Some(core) = weak.upgrade() else { return };
                    if !gather(&core.drives, event, &mut items, &mut drives) {
                        return;
                    }
                },
                _ = every.tick() => {
                    let Some(core) = weak.upgrade() else { return };
                    if std::mem::take(&mut drives) {
                        core.tell_drives().await;
                    }
                    for (id, ids) in std::mem::take(&mut items) {
                        if let Some(drive) = core.drives.get(&id) {
                            core.tell_items(&drive, &ids).await;
                        }
                    }
                }
            }
        }
    });
    Ok(())
}

/// Adds what `event` tells to what is sent next (`items` by drive, and whether the drives
/// changed); `false` once the events end. Events missed (the listener lagged) send the drives
/// and every item each drive tracks again, so Files never stays on a state an item left.
fn gather(
    set: &DriveSet,
    event: Result<DriveEvent, tokio::sync::broadcast::error::RecvError>,
    items: &mut BTreeMap<String, BTreeSet<NodeId>>,
    drives: &mut bool,
) -> bool {
    match event {
        Ok(DriveEvent::Drives) => *drives = true,
        Ok(DriveEvent::Items { drive, ids }) => {
            items.entry(drive).or_default().extend(ids);
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            *drives = true;
            for drive in set.all() {
                items
                    .entry(drive.id.clone())
                    .or_default()
                    .extend(drive.drive.tracked());
            }
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
    }
    true
}

/// The absolute path of `id` on `drive`, for tests and callers that hold a node.
pub fn absolute(drive: &LiveDrive, id: &NodeId) -> Option<PathBuf> {
    drive
        .drive
        .path_of(id)
        .ok()
        .flatten()
        .map(|inside| drive.path.join(inside))
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_drive::testing::{MemoryListing, MemorySource};
    use porter_drive::{Drive, DriveConfig, RemoteId};
    use tokio::sync::broadcast::error::RecvError;

    #[tokio::test]
    async fn events_missed_send_every_tracked_item_again() {
        let dir = test_kit::Scratch::new("storage1-gather");
        let listing = Arc::new(MemoryListing::default());
        let source = Arc::new(MemorySource::default());
        listing.file("a", None, "a.bin", 4, "1");
        source.put("a", b"aaaa".to_vec());
        source.hold();
        let drive = Drive::open(
            DriveConfig::new(dir.join("cache")),
            listing.clone(),
            source.clone(),
        )
        .await
        .expect("drive");
        let a = NodeId::Remote(RemoteId("a".into()));
        drive.open_file(&a).expect("open");
        let set = DriveSet::default();
        set.insert(Arc::new(LiveDrive::new(
            "one".into(),
            "/org/quire/Accounts1/account/one".into(),
            "ada@example.test".into(),
            "OneDrive",
            dir.join("Cloud"),
            drive.clone(),
        )));
        let (mut items, mut drives) = (BTreeMap::new(), false);
        assert!(gather(
            &set,
            Err(RecvError::Lagged(9)),
            &mut items,
            &mut drives
        ));
        assert!(drives);
        assert_eq!(items.get("one"), Some(&BTreeSet::from([a.clone()])));
        assert!(!gather(
            &set,
            Err(RecvError::Closed),
            &mut items,
            &mut drives
        ));
        source.release();
        drive.release(&a);
    }
}
