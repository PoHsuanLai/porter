//! Sending what was changed here, one change at a time, oldest first.
//!
//! - **What is ready.** A file is sent once nothing has it open for writing and it has been left
//!   alone for the quiet time; a folder made here before what is in it; a move once the folder
//!   it goes to is on the drive; a folder's removal after what was in it on the drive has left
//!   it. What stays on this computer (see [`crate::local_only`]) is never sent.
//! - **Holding.** Nothing is sent while paused or offline. A change the drive could not take now
//!   is tried again after a wait that doubles up to [`BACKOFF_MOST`]; one it refused (no room,
//!   no right to change it, a name it does not take) keeps its bytes here with the reason, and is
//!   tried again on the next listing change or when it is changed again. One in use on the drive
//!   waits with a reason that says so. A sign-in the drive refused holds everything until it is
//!   signed in again ([`Drive::set_signed_out`]).
//! - **A lost answer.** A step the drive took whose answer never came is sent again, and the
//!   drive refuses it as a conflict: an item made under the id reserved for it (on a drive that
//!   reserves ids, kept with the change before the first try), or one that holds just the bytes
//!   sent (its size and hash), is taken as what the step made, never made twice. A rename onto a
//!   name the drive gave something else meanwhile takes the next free number (" 2") and is sent
//!   again after a wait.
//! - **A snapshot.** An upload sends a copy of the bytes taken when it begins (a clone sharing
//!   their blocks where the file system makes one), so what is written meanwhile never mixes
//!   into what is sent. Its session belongs to that generation of the bytes: one opened for
//!   older bytes is given up, never resumed with newer ones.
//! - **After.** The snapshot sent becomes the cache's copy of the version the drive gave it; the
//!   bytes here are taken as sent only when they are still the generation it was taken of. An
//!   item made here takes the drive's id (keeping its inode number and path), and what is in it
//!   follows; one removed here while it was sent goes to the recycle bin. The change stays laid
//!   over the listing until a listing pass shows it.

use super::{Drive, Inner, NodeId, held};
use crate::Reason;
use crate::index::{Awaiting, Base, Edit, Index, Row};
use crate::naming;
use crate::ranges::Ranges;
use crate::sink::Sink;
use porter_sync::{
    BaseVersion, Conflict, Content, Place, Progress, PutRefused, RemoteId, RemoteSide,
    RemoteVersion, RetryAfter, Session, Upload, UploadTo, Written,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// The first wait after the drive could not take a change.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);

/// The longest wait between tries.
const BACKOFF_MOST: Duration = Duration::from_secs(300);

/// Listing passes an item sent here may be missing from before it is taken as gone.
const PASSES: u32 = 3;

/// The change being sent: which, and how far its bytes are.
#[derive(Debug, Clone)]
pub(crate) struct Sent {
    pub(super) id: NodeId,
    pub(super) sent: u64,
    pub(super) size: u64,
}

/// One step to send.
#[derive(Debug)]
enum Op {
    Folder {
        id: NodeId,
        place: Place,
    },
    Upload {
        id: NodeId,
        to: UploadTo,
        base: BaseVersion,
        /// The number of the bytes here.
        file: u64,
        size: u64,
        /// The generation of the bytes it sends.
        generation: u64,
        resume: Option<Session>,
        before: Option<String>,
        /// The copy it sends, once taken.
        snapshot: Option<PathBuf>,
    },
    Relocate {
        id: NodeId,
        item: RemoteId,
        base: BaseVersion,
        place: Place,
        before: Option<String>,
    },
    Trash {
        id: NodeId,
        item: RemoteId,
        base: BaseVersion,
    },
}

impl Op {
    fn id(&self) -> &NodeId {
        match self {
            Op::Folder { id, .. }
            | Op::Upload { id, .. }
            | Op::Relocate { id, .. }
            | Op::Trash { id, .. } => id,
        }
    }
}

/// What the sender does next.
enum Next {
    Send(Op),
    /// Nothing is ready before this time (`None`: until something changes).
    Wait(Option<Instant>),
}

/// The folder `parent` as the drive names it, when it is on the drive.
fn place_of(parent: &NodeId, name: &str) -> Option<Place> {
    match parent {
        NodeId::Top => Some(Place::new(None, name)),
        NodeId::Remote(id) => Some(Place::new(Some(id.clone()), name)),
        NodeId::Local(_) => None,
    }
}

/// Where the item made here as `edit` goes, under the id reserved for it.
fn new_place(edit: &Edit) -> Option<Place> {
    let mut place = place_of(&edit.parent, &edit.name)?;
    place.id = edit.reserved.clone().map(RemoteId);
    Some(place)
}

/// Tells the index of the upload session and the bytes sent, for the bytes of one generation.
struct Told {
    weak: Weak<Inner>,
    id: NodeId,
    generation: u64,
}

impl Progress for Told {
    fn session(&self, session: Option<&Session>) {
        let Some(inner) = self.weak.upgrade() else {
            return;
        };
        {
            let mut index = held(&inner.index);
            let edit = index.edits.get_mut(&self.id);
            let current = edit
                .as_ref()
                .and_then(|edit| edit.dirty.as_ref())
                .map(|d| d.generation);
            match (session, edit) {
                (Some(session), Some(edit)) if current == Some(self.generation) => {
                    edit.session = Some(session.clone());
                    edit.session_of = self.generation;
                }
                // Opened for bytes that changed meanwhile: given up once this upload ends.
                (Some(session), _) => held(&inner.abandoned).push(session.clone()),
                (None, Some(edit)) if edit.session_of == self.generation => {
                    edit.session = None;
                    edit.session_of = 0;
                }
                (None, _) => {}
            }
        }
        inner.touched();
        // On the disk at once (an upload resumes after a crash), off the runtime's threads.
        let writer = Arc::clone(&inner);
        drop(inner.runtime.spawn_blocking(move || {
            if let Err(why) = writer.flush_now() {
                eprintln!("porter-drive: the cache's index could not be written: {why}");
            }
        }));
    }

    fn sent(&self, bytes: u64) {
        let Some(inner) = self.weak.upgrade() else {
            return;
        };
        if let Some(sent) = held(&inner.sending).as_mut() {
            sent.sent = bytes;
        }
        inner.emit(vec![self.id.clone()]);
    }

    fn held(&self) -> bool {
        self.weak
            .upgrade()
            .is_some_and(|inner| inner.paused.load(Ordering::SeqCst))
    }
}

impl Inner {
    /// Whether `edit`'s file may be sent now, or when it may.
    fn quiet_since(&self, id: &NodeId) -> Result<(), Option<Instant>> {
        match held(&self.writing).get(id) {
            Some(writing) if writing.handles > 0 => Err(None),
            Some(writing) => {
                let at = writing.last + self.quiet;
                match at <= Instant::now() {
                    true => Ok(()),
                    false => Err(Some(at)),
                }
            }
            None => Ok(()),
        }
    }

    /// Whether something still to be sent has its place on the drive in the folder `id`.
    fn still_in(index: &Index, id: &RemoteId) -> bool {
        index.edits.values().any(|edit| {
            edit.pending()
                && edit
                    .base
                    .as_ref()
                    .is_some_and(|base| base.parent.as_deref() == Some(id.0.as_str()))
        })
    }

    /// The step `edit` needs now; `Err` with when it may be ready, or `None` for not yet.
    fn op_for(&self, index: &Index, edit: &Edit) -> Result<Op, Option<Instant>> {
        let id = edit.id.clone();
        let listed = |item: &RemoteId| {
            self.listing
                .entry(item)
                .ok()
                .flatten()
                .map(|entry| entry.version.0)
        };
        let Some(base) = &edit.base else {
            let place = new_place(edit).ok_or(None)?;
            if edit.folder {
                return Ok(Op::Folder { id, place });
            }
            let dirty = edit.dirty.as_ref().ok_or(None)?;
            self.quiet_since(&id)?;
            return Ok(Op::Upload {
                to: UploadTo::New(place),
                base: BaseVersion::Absent,
                file: dirty.file,
                size: dirty.size,
                generation: dirty.generation,
                resume: Self::resumed(edit),
                before: None,
                snapshot: None,
                id,
            });
        };
        let NodeId::Remote(item) = &id else {
            return Err(None);
        };
        let at = BaseVersion::At(RemoteVersion(base.version.clone()));
        if edit.removed {
            if edit.folder && Self::still_in(index, item) {
                return Err(None);
            }
            return Ok(Op::Trash {
                item: item.clone(),
                base: at,
                id,
            });
        }
        if !edit.in_place() {
            let place = place_of(&edit.parent, &edit.name).ok_or(None)?;
            return Ok(Op::Relocate {
                item: item.clone(),
                base: at,
                place,
                before: listed(item),
                id,
            });
        }
        let dirty = edit.dirty.as_ref().ok_or(None)?;
        self.quiet_since(&id)?;
        Ok(Op::Upload {
            to: UploadTo::Replace(item.clone()),
            base: at,
            file: dirty.file,
            size: dirty.size,
            generation: dirty.generation,
            resume: Self::resumed(edit),
            before: listed(item),
            snapshot: None,
            id,
        })
    }

    /// The session `edit`'s upload resumes: only one opened for the bytes it has now.
    fn resumed(edit: &Edit) -> Option<Session> {
        let generation = edit.dirty.as_ref()?.generation;
        edit.session
            .clone()
            .filter(|_| edit.session_of == generation)
    }

    /// Takes the copy an upload of `id`'s bytes `local/<n>` of `generation` sends: the copy and
    /// its size, or `None` when the bytes changed since (the step is looked at again). The copy
    /// is made without the bytes' lock, which a write waits on (a large file's copy takes long),
    /// and kept only when the bytes are still that generation under the lock after it.
    fn take_snapshot(
        &self,
        id: &NodeId,
        n: u64,
        generation: u64,
    ) -> std::io::Result<Option<(PathBuf, u64)>> {
        let current = || {
            held(&self.index)
                .edits
                .get(id)
                .and_then(|edit| edit.dirty.clone())
                .filter(|d| d.file == n && d.generation == generation)
        };
        let Some(dirty) = current() else {
            return Ok(None);
        };
        let to = self.local_file(n).with_extension("sending");
        let copied = super::write::clone_file(&self.local_file(n), &to, dirty.size);
        #[cfg(test)]
        let copied = copied.and_then(|()| self.fault("snapshot"));
        #[cfg(test)]
        self.at_hook("snapshot");
        let lock = self.file_lock(n);
        let _bytes = held(&lock);
        match (copied, current()) {
            (Ok(()), Some(_)) => Ok(Some((to, dirty.size))),
            (copied, now) => {
                let _ = std::fs::remove_file(&to);
                match (copied, now) {
                    // Written, sent or removed meanwhile: what it is now is looked at again.
                    (_, None) => Ok(None),
                    (Err(why), Some(_)) => Err(why),
                    (Ok(()), Some(_)) => Ok(None),
                }
            }
        }
    }

    /// The next step to send, oldest change first among those ready.
    fn next(&self) -> Next {
        if self.sink.is_none()
            || self.paused.load(Ordering::SeqCst)
            || self.offline.load(Ordering::SeqCst)
            || self.signed_out.load(Ordering::SeqCst)
        {
            return Next::Wait(None);
        }
        if let Some((until, _)) = *held(&self.backoff)
            && until > Instant::now()
        {
            return Next::Wait(Some(until));
        }
        let edits: Vec<Edit> = {
            let index = held(&self.index);
            let mut edits: Vec<Edit> = index
                .edits
                .values()
                .filter(|edit| edit.pending() && edit.refused.is_none())
                .cloned()
                .collect();
            let stale = held(&self.stale);
            edits.retain(|edit| !stale.contains(&edit.id));
            edits.sort_by_key(|edit| edit.seq);
            edits
        };
        let mut soonest: Option<Instant> = None;
        for edit in &edits {
            if self.local_only(&edit.id) {
                continue;
            }
            let op = {
                let index = held(&self.index);
                self.op_for(&index, edit)
            };
            match op {
                Ok(op) => return Next::Send(op),
                Err(Some(at)) => soonest = Some(soonest.map_or(at, |s| s.min(at))),
                Err(None) => {}
            }
        }
        Next::Wait(soonest)
    }

    /// Waits longer before the next try: the drive could not take a change now.
    fn back_off(&self, asked: Duration) {
        let mut backoff = held(&self.backoff);
        let wait = backoff
            .map_or(BACKOFF_FIRST, |(_, last)| last * 2)
            .max(asked)
            .min(BACKOFF_MOST);
        *backoff = Some((Instant::now() + wait, wait));
    }

    /// The drive took `op` (with `made`, only the item it makes: an earlier try made it with
    /// other bytes than these): the index says what the drive has now. The index is written to
    /// the disk after, by the sender, off the runtime's threads.
    fn landed(self: &Arc<Self>, op: Op, written: Option<Written>, made: bool) {
        // The bytes' lock first: no write is half-noted while they are taken as sent.
        let lock = match &op {
            Op::Upload { file, .. } => Some(self.file_lock(*file)),
            _ => None,
        };
        let bytes = lock.as_ref().map(|lock| held(lock));
        let mut changed = vec![op.id().clone()];
        let mut aliased = None;
        {
            let mut index = held(&self.index);
            match index.edits.remove(op.id()) {
                None => {
                    // Removed here while it was sent, or saved over another file: what the
                    // drive made of it goes to the recycle bin.
                    if let (
                        Op::Folder { place, .. }
                        | Op::Upload {
                            to: UploadTo::New(place),
                            ..
                        },
                        Some(written),
                    ) = (&op, &written)
                    {
                        let mut gone = Self::removed_there(&op, place, written);
                        gone.seq = index.take_seq();
                        changed.push(gone.id.clone());
                        index.edits.insert(gone.id.clone(), gone);
                    }
                    if let Op::Upload {
                        snapshot: Some(path),
                        ..
                    } = &op
                    {
                        let _ = std::fs::remove_file(path);
                    }
                }
                Some(mut edit) => {
                    self.took(&mut index, &mut edit, &op, written.as_ref(), made);
                    if let (NodeId::Local(n), NodeId::Remote(_)) = (op.id(), &edit.id) {
                        aliased = Some((*n, edit.id.clone()));
                    }
                    changed.push(edit.id.clone());
                    index.edits.insert(edit.id.clone(), edit);
                }
            }
        }
        drop(bytes);
        if let Some((n, id)) = aliased {
            self.alias(n, &id);
        }
        *held(&self.backoff) = None;
        self.touched();
        self.wanted.notify_one();
        self.wake.notify_one();
        self.emit(changed);
    }

    /// What the drive made of `op` at `place`, which was removed here while it was sent: its
    /// removal, to send.
    fn removed_there(op: &Op, place: &Place, written: &Written) -> Edit {
        let (folder, size) = match op {
            Op::Upload { size, .. } => (false, *size),
            _ => (true, 0),
        };
        let base = Base {
            version: written.version.0.clone(),
            parent: place.parent.clone().map(|p| p.0),
            name: place.name.clone(),
            size,
            modified: None,
        };
        let parent = place.parent.clone().map_or(NodeId::Top, NodeId::Remote);
        let mut gone = Edit::of_base(
            NodeId::Remote(written.id.clone()),
            folder,
            parent,
            base,
            super::now_secs(),
        );
        gone.removed = true;
        gone
    }

    /// Notes in `edit` what the drive has after it took `op`, under the index's lock (and an
    /// upload's bytes' lock). With `made`, the upload made the item and not with these bytes:
    /// it takes the drive's id and version, and its bytes are sent again over them.
    fn took(
        &self,
        index: &mut Index,
        edit: &mut Edit,
        op: &Op,
        written: Option<&Written>,
        made: bool,
    ) {
        // A removal made meanwhile is still to be sent, against the version the drive has now.
        let awaiting = |before: Option<String>, edit: &Edit| {
            (!edit.removed).then_some(Awaiting { before, passes: 0 })
        };
        match (op, written) {
            (Op::Trash { .. }, _) => {
                edit.awaiting = Some(Awaiting {
                    before: None,
                    passes: 0,
                });
            }
            (
                Op::Folder { place, .. } | Op::Relocate { place, .. },
                Some(Written { id, version }),
            ) => {
                let before = match op {
                    Op::Relocate { before, .. } => before.clone(),
                    _ => None,
                };
                edit.id = NodeId::Remote(id.clone());
                edit.base = Some(Base {
                    version: version.0.clone(),
                    parent: place.parent.clone().map(|p| p.0),
                    name: place.name.clone(),
                    size: edit.base.as_ref().map_or(0, |b| b.size),
                    modified: edit.base.as_ref().and_then(|b| b.modified),
                });
                edit.awaiting = awaiting(before, edit);
            }
            (
                Op::Upload {
                    to,
                    size,
                    generation,
                    file,
                    before,
                    snapshot,
                    ..
                },
                Some(Written { id, version }),
            ) => {
                let place = match to {
                    UploadTo::New(place) => Some(place.clone()),
                    UploadTo::Replace(_) => None,
                };
                let old = edit.base.clone();
                edit.id = NodeId::Remote(id.clone());
                if edit.session_of == *generation {
                    edit.session = None;
                    edit.session_of = 0;
                }
                edit.base = Some(Base {
                    version: version.0.clone(),
                    parent: match &place {
                        Some(place) => place.parent.clone().map(|p| p.0),
                        None => old.as_ref().and_then(|b| b.parent.clone()),
                    },
                    name: match &place {
                        Some(place) => place.name.clone(),
                        None => old.as_ref().map_or_else(String::new, |b| b.name.clone()),
                    },
                    size: *size,
                    modified: edit.dirty.as_ref().map(|d| d.modified),
                });
                edit.awaiting = awaiting(before.clone(), edit);
                if made {
                    // The drive holds an earlier try's bytes: these are sent again over them.
                    self.abandon(edit);
                    if let Some(snapshot) = snapshot {
                        let _ = std::fs::remove_file(snapshot);
                    }
                    return;
                }
                // The copy sent is the cache's copy of the new version.
                if let Some(snapshot) = snapshot {
                    let to = self.content(id, &version.0);
                    if std::fs::rename(snapshot, &to).is_err() {
                        let _ = std::fs::remove_file(snapshot);
                    } else {
                        if let Some(old) = index.items.remove(&id.0)
                            && old.version != version.0
                        {
                            let _ = std::fs::remove_file(self.content(id, &old.version));
                        }
                        let clock = index.clock;
                        index.items.insert(
                            id.0.clone(),
                            Row {
                                version: version.0.clone(),
                                size: *size,
                                present: Ranges::whole(*size),
                                opened: clock,
                                checked: true,
                            },
                        );
                        self.wrote(&to);
                    }
                }
                // The bytes here were sent only if they are still the ones the copy was taken
                // of; written since, they are sent again.
                let sent = edit
                    .dirty
                    .as_ref()
                    .is_some_and(|d| d.file == *file && d.generation == *generation);
                if sent && !edit.removed {
                    edit.dirty = None;
                    self.drop_file(*file);
                }
            }
            _ => {}
        }
    }

    /// The drive refused `op` for good (for now): the change keeps its bytes here and says why.
    fn refused(&self, op: &Op, reason: Reason) {
        if let Some(edit) = held(&self.index).edits.get_mut(op.id()) {
            edit.refused = Some(reason);
        }
        self.touched();
        self.emit(vec![op.id().clone()]);
    }

    /// Settles what a listing pass shows was sent: a change the listing has caught up with is no
    /// longer laid over it, and what the drive refused is tried again.
    pub(super) fn settle(&self) {
        let mut changed: Vec<NodeId> = held(&self.index)
            .edits
            .values_mut()
            .filter_map(|edit| edit.refused.take().map(|_| edit.id.clone()))
            .collect();
        let ids: Vec<NodeId> = held(&self.index).edits.keys().cloned().collect();
        for id in ids {
            let NodeId::Remote(item) = &id else {
                continue;
            };
            let Ok(listed) = self.listing.entry(item) else {
                continue;
            };
            let mut index = held(&self.index);
            let Some(edit) = index.edits.get_mut(&id) else {
                continue;
            };
            let Some(awaiting) = edit.awaiting.as_mut() else {
                continue;
            };
            let settled = match (&listed, edit.removed) {
                (None, true) => true,
                (Some(_), true) | (None, false) => {
                    awaiting.passes += 1;
                    awaiting.passes >= PASSES
                }
                (Some(entry), false) => {
                    let ours = edit.base.as_ref().map(|b| b.version.as_str());
                    Some(entry.version.0.as_str()) == ours
                        || Some(&entry.version.0) != awaiting.before.as_ref()
                }
            };
            if !settled {
                continue;
            }
            if edit.pending() {
                edit.awaiting = None;
                continue;
            }
            // Removed here while open: its bytes stay until it is closed.
            if let Some(dirty) = &edit.dirty {
                let file = dirty.file;
                if self.is_open(&id) {
                    continue;
                }
                self.drop_file(file);
            }
            index.edits.remove(&id);
            changed.push(id);
        }
        self.touched();
        self.wake.notify_one();
        self.emit(changed);
    }
}

/// `op` with the copy it sends taken, off the runtime's threads (a copy waits on the disk);
/// `None` when the bytes changed since it was chosen, or no copy could be taken (tried again
/// after a wait).
async fn snapshot(inner: &Arc<Inner>, mut op: Op) -> Option<Op> {
    let Op::Upload {
        id,
        file,
        generation,
        ..
    } = &op
    else {
        return Some(op);
    };
    let (id, file, generation) = (id.clone(), *file, *generation);
    let taker = Arc::clone(inner);
    let taken =
        tokio::task::spawn_blocking(move || taker.take_snapshot(&id, file, generation)).await;
    match taken {
        Ok(Ok(Some((path, taken_size)))) => {
            if let Op::Upload { snapshot, size, .. } = &mut op {
                *snapshot = Some(path);
                *size = taken_size;
            }
            Some(op)
        }
        Ok(Ok(None)) => None,
        Ok(Err(why)) => {
            eprintln!("porter-drive: a file to send could not be copied: {why}");
            if why.kind() == std::io::ErrorKind::StorageFull {
                held(&inner.waiting).insert(op.id().clone(), Reason::NoRoomHere);
                inner.emit(vec![op.id().clone()]);
            }
            inner.back_off(Duration::ZERO);
            None
        }
        Err(_) => {
            inner.back_off(Duration::ZERO);
            None
        }
    }
}

/// `op` with an id reserved for the item it makes, on a drive that reserves them: kept with the
/// change and on the disk before the first try, so every try makes the same item. `None` when
/// none could be had now (tried again after a wait) or the change went meanwhile.
async fn reserved(inner: &Arc<Inner>, sink: &dyn Sink, mut op: Op) -> Option<Op> {
    let id = op.id().clone();
    let place = match &mut op {
        Op::Folder { place, .. }
        | Op::Upload {
            to: UploadTo::New(place),
            ..
        } => place,
        _ => return Some(op),
    };
    if place.id.is_some() {
        return Some(op);
    }
    let given = match sink.reserve_ids(1).await {
        Ok(ids) => ids.into_iter().next(),
        Err(PutRefused::Transient(after)) => {
            inner.back_off(Duration::from_secs(u64::from(after.0)));
            return None;
        }
        // Sent with none: the step itself meets what refused it.
        Err(_) => None,
    };
    let Some(given) = given else {
        return Some(op);
    };
    inner.change(&id, |edit, _| edit.reserved = Some(given.0.clone()))?;
    inner.touched();
    let writer = Arc::clone(inner);
    let written = tokio::task::spawn_blocking(move || writer.flush_now()).await;
    if !matches!(written, Ok(Ok(()))) {
        inner.back_off(Duration::ZERO);
        return None;
    }
    place.id = Some(given);
    Some(op)
}

/// What the drive made of a step on an earlier try whose answer was lost.
enum Adopted {
    /// Just what the step sends: it is taken as sent.
    Sent(Written),
    /// The item the step makes, under the id reserved for it, with bytes of an earlier try:
    /// it takes the id, and these bytes are sent again over it.
    Made(Written),
}

/// What the drive made of `op` on an earlier try whose answer was lost, when `conflict` names
/// it: the item made under the id reserved for it, or one that holds just what `op` sends (the
/// same size and content hash) and that no other change here is of.
async fn adopted(
    inner: &Arc<Inner>,
    sink: &dyn Sink,
    op: &Op,
    conflict: &Conflict,
) -> Option<Adopted> {
    let (there, version) = match (op, &conflict.remote) {
        (
            Op::Folder { .. }
            | Op::Upload {
                to: UploadTo::New(_),
                ..
            },
            RemoteSide::Exists(id, version),
        ) => (id, version),
        (
            Op::Upload {
                to: UploadTo::Replace(id),
                ..
            },
            RemoteSide::Changed(version),
        ) => (id, version),
        _ => return None,
    };
    let reserved = match op {
        Op::Folder { place, .. }
        | Op::Upload {
            to: UploadTo::New(place),
            ..
        } => place.id.as_ref() == Some(there),
        _ => false,
    };
    let made = Written {
        id: there.clone(),
        version: version.clone(),
    };
    if reserved && matches!(op, Op::Folder { .. }) {
        return Some(Adopted::Sent(made));
    }
    // Another change here is of that item: it is not what this step made.
    let theirs = NodeId::Remote(there.clone());
    if !reserved && *op.id() != theirs && held(&inner.index).edits.contains_key(&theirs) {
        return None;
    }
    let same = same_bytes(inner, sink, op, there).await;
    match (same, reserved) {
        (Some(version), _) => Some(Adopted::Sent(Written {
            id: there.clone(),
            version,
        })),
        (None, true) => Some(Adopted::Made(made)),
        (None, false) => None,
    }
}

/// The version of the drive's `there` when it holds just what the upload `op` sends (its size
/// and content hash).
async fn same_bytes(
    inner: &Arc<Inner>,
    sink: &dyn Sink,
    op: &Op,
    there: &RemoteId,
) -> Option<RemoteVersion> {
    let Op::Upload {
        snapshot: Some(path),
        size,
        ..
    } = op
    else {
        return None;
    };
    let found = sink.look(there).await.ok().flatten()?;
    let theirs = found
        .hash
        .filter(|_| !found.folder && found.size == *size)?;
    let (kind, path) = (inner.source.hashes(), path.clone());
    let ours = tokio::task::spawn_blocking(move || crate::hash::of_file(kind, &path))
        .await
        .ok()?
        .ok()??;
    (ours == theirs.0).then_some(found.version)
}

/// Sends what was changed here, one step at a time, for as long as the drive lives.
pub(super) async fn sender(weak: Weak<Inner>, wake: Arc<Notify>) {
    loop {
        let next = {
            let Some(inner) = weak.upgrade() else { return };
            inner.next()
        };
        let op = match next {
            Next::Send(op) => op,
            Next::Wait(None) => {
                wake.notified().await;
                continue;
            }
            Next::Wait(Some(at)) => {
                let _ = tokio::time::timeout_at(at.into(), wake.notified()).await;
                continue;
            }
        };
        let Some(inner) = weak.upgrade() else { return };
        let Some(sink) = inner.sink.clone() else {
            return;
        };
        // Sessions of bytes that changed are given up first.
        let abandoned = std::mem::take(&mut *held(&inner.abandoned));
        for session in &abandoned {
            sink.abandon(session).await;
        }
        let Some(op) = reserved(&inner, &*sink, op).await else {
            continue;
        };
        let op = match op {
            Op::Upload { .. } => match snapshot(&inner, op).await {
                Some(op) => op,
                None => continue,
            },
            op => op,
        };
        let size = match &op {
            Op::Upload { size, .. } => *size,
            _ => 0,
        };
        *held(&inner.sending) = Some(Sent {
            id: op.id().clone(),
            sent: 0,
            size,
        });
        inner.emit(vec![op.id().clone()]);
        drop(inner);
        let told = Told {
            weak: weak.clone(),
            id: op.id().clone(),
            generation: match &op {
                Op::Upload { generation, .. } => *generation,
                _ => 0,
            },
        };
        let result = match &op {
            Op::Folder { place, .. } => sink.make_folder(place).await.map(Some),
            Op::Upload {
                to,
                base,
                snapshot: Some(snapshot),
                size,
                resume,
                ..
            } => {
                let content = Content::new(snapshot.clone(), *size);
                let upload = Upload {
                    to: to.clone(),
                    base: base.clone(),
                    content: &content,
                    resume: resume.clone(),
                    progress: &told,
                };
                sink.upload(upload).await.map(Some)
            }
            Op::Relocate {
                item, base, place, ..
            } => sink.relocate(item, base, place).await.map(Some),
            Op::Trash { item, base, .. } => sink.trash(item, base).await.map(|()| None),
            Op::Upload { snapshot: None, .. } => Err(PutRefused::Transient(RetryAfter(0))),
        };
        let Some(inner) = weak.upgrade() else { return };
        *held(&inner.sending) = None;
        held(&inner.waiting).remove(op.id());
        // A conflict may be this step's own earlier try, whose answer was lost.
        let mut made = false;
        let result = match result {
            Err(PutRefused::Conflict(conflict)) => {
                match adopted(&inner, &*sink, &op, &conflict).await {
                    Some(Adopted::Sent(written)) => Ok(Some(written)),
                    Some(Adopted::Made(written)) => {
                        made = true;
                        Ok(Some(written))
                    }
                    None => Err(PutRefused::Conflict(conflict)),
                }
            }
            other => other,
        };
        if result.is_err()
            && let Op::Upload {
                snapshot: Some(path),
                ..
            } = &op
        {
            let _ = std::fs::remove_file(path);
        }
        match result {
            Ok(written) => {
                inner.landed(op, written, made);
                // On the disk before the next step: what the drive has is never sent again.
                let writer = Arc::clone(&inner);
                if let Ok(Err(why)) = tokio::task::spawn_blocking(move || writer.flush_now()).await
                {
                    eprintln!("porter-drive: the cache's index could not be written: {why}");
                }
            }
            Err(PutRefused::Transient(after)) => {
                inner.back_off(Duration::from_secs(u64::from(after.0)));
                inner.emit(vec![op.id().clone()]);
            }
            Err(PutRefused::InUse(after)) => {
                held(&inner.waiting).insert(op.id().clone(), Reason::InUse);
                inner.back_off(Duration::from_secs(u64::from(after.0)));
                inner.emit(vec![op.id().clone()]);
            }
            Err(PutRefused::Unauthorized) => {
                // Held until it is signed in again; a listing pass says so to the person.
                inner.signed_out.store(true, Ordering::SeqCst);
                inner.wanted.notify_one();
                inner.emit(vec![op.id().clone()]);
            }
            Err(PutRefused::Quota) => inner.refused(&op, Reason::NoRoom),
            Err(PutRefused::Forbidden) => inner.refused(&op, Reason::MayNotWrite),
            Err(PutRefused::NameRefused) => inner.refused(&op, Reason::NameRefused),
            Err(PutRefused::Declined) => {
                // An id reserved and used up is given up with it: the next try reserves another.
                inner.change(op.id(), |edit, _| edit.reserved = None);
                inner.refused(&op, Reason::Declined);
            }
            Err(PutRefused::Conflict(conflict)) => inner.conflicted(&op, &conflict),
            // a refusal a later porter-sync adds: the drive declined it for a reason of its own
            Err(_) => inner.refused(&op, Reason::Declined),
        }
    }
}

impl Inner {
    /// The drive has another version than `op` was made against: held until a listing pass
    /// shows it, which meets both (see `conflict`). A name taken there is met at once.
    fn conflicted(&self, op: &Op, conflict: &Conflict) {
        let mut changed = vec![op.id().clone()];
        if let (Op::Relocate { item, .. }, RemoteSide::Exists(there, _)) = (op, &conflict.remote)
            && there != item
        {
            // The name was given to something else on the drive meanwhile: this one takes the
            // next free number there, sent after a wait.
            self.name_aside(op.id());
            self.back_off(Duration::ZERO);
            self.wake.notify_one();
            self.touched();
            self.emit(vec![op.id().clone()]);
            return;
        }
        if self.met_conflict(op.id(), conflict) {
            changed.push(self.canonical(op.id()));
            self.wake.notify_one();
        } else {
            held(&self.stale).insert(op.id().clone());
            self.wanted.notify_one();
        }
        self.touched();
        self.emit(changed);
    }
}

impl Inner {
    /// Renames `id` here to the next free number of its name among the drive's names in its
    /// folder (never the name it has).
    fn name_aside(&self, id: &NodeId) {
        let Some(edit) = self.edit(id) else {
            return;
        };
        let taken: Vec<String> = self
            .children(&edit.parent)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|node| self.node_name(&node.id))
            .collect();
        let name = naming::free(&edit.name, |candidate| {
            candidate == edit.name || taken.iter().any(|t| t == candidate)
        });
        self.change(id, |now, _| {
            if now.name == edit.name {
                now.name = name;
            }
        });
    }
}

impl Drive {
    /// Holds what waits to be sent (`true`), or sends it again.
    ///
    /// An upload under way stops at its next chunk, its session kept, and goes on from there.
    pub fn set_paused(&self, paused: bool) {
        let was = self.inner.paused.swap(paused, Ordering::SeqCst);
        if was && !paused {
            *held(&self.inner.backoff) = None;
            self.inner.wake.notify_one();
        }
        let ids: Vec<NodeId> = held(&self.inner.index).edits.keys().cloned().collect();
        self.inner.emit(ids);
    }

    /// Whether what waits to be sent is held.
    pub fn paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    /// Notes whether the drive refuses the account's sign-in (the listing says so too): while it
    /// does, nothing is sent; signed in again, what waits is sent.
    pub fn set_signed_out(&self, signed_out: bool) {
        if self.inner.signed_out.swap(signed_out, Ordering::SeqCst) && !signed_out {
            *held(&self.inner.backoff) = None;
            self.inner.wake.notify_one();
        }
    }

    /// Whether the drive refused the account's sign-in when it was last sent something.
    pub fn signed_out(&self) -> bool {
        self.inner.signed_out.load(Ordering::SeqCst)
    }

    /// Told each time a listing pass is wanted: a change was sent (the listing shows it on its
    /// next pass), or the drive has a newer version than a change was made against.
    pub fn listing_wanted(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.wanted)
    }

    /// Tries again at once what the drive refused or could not take.
    pub fn retry(&self) {
        let ids: Vec<NodeId> = {
            let mut index = held(&self.inner.index);
            index
                .edits
                .values_mut()
                .filter_map(|edit| edit.refused.take().map(|_| edit.id.clone()))
                .collect()
        };
        *held(&self.inner.backoff) = None;
        self.inner.touched();
        self.inner.wake.notify_one();
        self.inner.emit(ids);
    }

    /// How many changes made here are not on the drive yet.
    pub fn unsent(&self) -> usize {
        held(&self.inner.index)
            .edits
            .values()
            .filter(|edit| edit.pending())
            .count()
    }
}
