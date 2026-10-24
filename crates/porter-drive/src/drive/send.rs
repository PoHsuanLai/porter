//! Sending what was changed here, one change at a time, oldest first.
//!
//! - **What is ready.** A file is sent once nothing has it open for writing and it has been left
//!   alone for the quiet time; a folder made here before what is in it; a move once the folder
//!   it goes to is on the drive; a folder's removal after what was in it on the drive has left
//!   it. What stays on this computer (see [`crate::local_only`]) is never sent.
//! - **Holding.** Nothing is sent while paused or offline. A change the drive could not take now
//!   is tried again after a wait that doubles up to [`BACKOFF_MOST`]; one it refused (no room,
//!   no right to change it) keeps its bytes here with the words, and is tried again on the next
//!   listing change or when it is changed again.
//! - **After.** A file sent becomes the cache's copy of the version the drive gave it; an item
//!   made here takes the drive's id (keeping its inode number and path), and what is in it
//!   follows. The change stays laid over the listing until a listing pass shows it.

use super::{Drive, Inner, NodeId, held};
use crate::index::{Awaiting, Edit, Index, Row};
use crate::ranges::Ranges;
use crate::words;
use porter_sync::{
    BaseVersion, Conflict, Content, Place, Progress, PutRefused, RemoteId, RemoteVersion, Session,
    Upload, UploadTo, Written,
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
        file: PathBuf,
        size: u64,
        writes: u64,
        resume: Option<Session>,
        before: Option<String>,
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
        NodeId::Top => Some(Place {
            parent: None,
            name: name.to_owned(),
        }),
        NodeId::Remote(id) => Some(Place {
            parent: Some(id.clone()),
            name: name.to_owned(),
        }),
        NodeId::Local(_) => None,
    }
}

/// Tells the index of the upload session and the bytes sent.
struct Told {
    weak: Weak<Inner>,
    id: NodeId,
}

impl Progress for Told {
    fn session(&self, session: Option<&Session>) {
        let Some(inner) = self.weak.upgrade() else {
            return;
        };
        if let Some(edit) = held(&inner.index).edits.get_mut(&self.id) {
            edit.session = session.cloned();
        }
        inner.touched();
        // On the disk at once: an upload resumes after a crash.
        if let Err(why) = inner.flush_now() {
            eprintln!("porter-drive: the cache's index could not be written: {why}");
        }
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
            let place = place_of(&edit.parent, &edit.name).ok_or(None)?;
            if edit.folder {
                return Ok(Op::Folder { id, place });
            }
            let dirty = edit.dirty.as_ref().ok_or(None)?;
            self.quiet_since(&id)?;
            return Ok(Op::Upload {
                to: UploadTo::New(place),
                base: BaseVersion::Absent,
                file: self.local_file(dirty.file),
                size: dirty.size,
                writes: dirty.writes,
                resume: edit.session.clone(),
                before: None,
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
            file: self.local_file(dirty.file),
            size: dirty.size,
            writes: dirty.writes,
            resume: edit.session.clone(),
            before: listed(item),
            id,
        })
    }

    /// The next step to send, oldest change first among those ready.
    fn next(&self) -> Next {
        if self.sink.is_none()
            || self.paused.load(Ordering::SeqCst)
            || self.offline.load(Ordering::SeqCst)
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

    /// The drive took `op`: the index says what the drive has now.
    fn landed(self: &Arc<Self>, op: Op, written: Option<Written>) {
        let mut changed = vec![op.id().clone()];
        let local = match op.id() {
            NodeId::Local(n) => Some(*n),
            _ => None,
        };
        {
            let mut index = held(&self.index);
            let Some(mut edit) = index.edits.remove(op.id()) else {
                return;
            };
            match (&op, &written) {
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
                    let before = match &op {
                        Op::Relocate { before, .. } => before.clone(),
                        _ => None,
                    };
                    edit.id = NodeId::Remote(id.clone());
                    edit.base = Some(crate::index::Base {
                        version: version.0.clone(),
                        parent: place.parent.clone().map(|p| p.0),
                        name: place.name.clone(),
                        size: edit.base.as_ref().map_or(0, |b| b.size),
                        modified: edit.base.as_ref().and_then(|b| b.modified),
                    });
                    edit.awaiting = Some(Awaiting { before, passes: 0 });
                }
                (
                    Op::Upload {
                        to,
                        size,
                        writes,
                        before,
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
                    edit.session = None;
                    let modified = edit.dirty.as_ref().map(|d| d.modified);
                    edit.base = Some(crate::index::Base {
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
                        modified,
                    });
                    edit.awaiting = Some(Awaiting {
                        before: before.clone(),
                        passes: 0,
                    });
                    // The bytes sent are the cache's copy of the new version, unless they
                    // changed while they went.
                    if let Some(dirty) = edit.dirty.clone()
                        && dirty.writes == *writes
                    {
                        let from = self.local_file(dirty.file);
                        let to = self.content(id, &version.0);
                        if std::fs::rename(&from, &to).is_ok() {
                            edit.dirty = None;
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
                }
                _ => {}
            }
            changed.push(edit.id.clone());
            index.edits.insert(edit.id.clone(), edit);
        }
        if let Some(n) = local
            && let Some(id) = changed.get(1).cloned()
        {
            self.alias(n, &id);
        }
        *held(&self.backoff) = None;
        self.touched();
        // On the disk at once: what the drive has is never sent again.
        if let Err(why) = self.flush_now() {
            eprintln!("porter-drive: the cache's index could not be written: {why}");
        }
        self.wanted.notify_one();
        self.emit(changed);
    }

    /// The drive refused `op` for good (for now): the change keeps its bytes here and says why.
    fn refused(&self, op: &Op, words: &str) {
        if let Some(edit) = held(&self.index).edits.get_mut(op.id()) {
            edit.refused = Some(words.to_owned());
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
            index.edits.remove(&id);
            changed.push(id);
        }
        self.touched();
        self.wake.notify_one();
        self.emit(changed);
    }
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
        };
        let result = match &op {
            Op::Folder { place, .. } => sink.make_folder(place).await.map(Some),
            Op::Upload {
                to,
                base,
                file,
                size,
                resume,
                ..
            } => {
                let content = Content::new(file.clone(), *size);
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
        };
        let Some(inner) = weak.upgrade() else { return };
        *held(&inner.sending) = None;
        match result {
            Ok(written) => inner.landed(op, written),
            Err(PutRefused::Transient(after)) => {
                inner.back_off(Duration::from_secs(u64::from(after.0)));
                inner.emit(vec![op.id().clone()]);
            }
            Err(PutRefused::Quota) => inner.refused(&op, words::NO_ROOM),
            Err(PutRefused::Forbidden) => inner.refused(&op, words::MAY_NOT_WRITE),
            Err(PutRefused::Conflict(conflict)) => inner.conflicted(&op, &conflict),
        }
    }
}

impl Inner {
    /// The drive has another version than `op` was made against: held until a listing pass
    /// shows it, which meets both (see `conflict`). A name taken there is met at once.
    fn conflicted(&self, op: &Op, conflict: &Conflict) {
        let mut changed = vec![op.id().clone()];
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

impl Drive {
    /// Holds what waits to be sent (`true`), or sends it again.
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
