//! The source seam: where a collection's items come from.
//!
//! Everything downstream of a source is shared: the vdir writer ([`super::PimMirror`]), the
//! journal, the `PullOnly` rule, the rescan and the supervisor. A source has two jobs and two
//! traits:
//!
//! - [`PimSource`], per account and kind: the collections there are (id, display name, colour,
//!   as a [`Found`]) and a [`Feed`] for each.
//! - [`Feed`], per collection: the changes since a cursor, as upserts that carry the item's
//!   bytes (`.ics` for a VEVENT or a VTODO, `.vcf` for a vCard) and deletes. The cursor type is
//!   the source's own ([`FeedCursor`]: a sync token, a delta link, a page token); the journal
//!   stores it as the opaque anchor text.
//!
//! [`FeedReplica`] makes any feed a porter-sync `Replica` for the engine, which is why nothing
//! else in syncd changed. The source is chosen by the capability's transport
//! ([`choose`]: `caldav` and `carddav` are [`dav`], `graph` is [`graph`]), never by provider.
//!
//! **A new source is one module** (`google` for `google_api`): a `PimSource` and a `Feed`, one
//! more arm in [`choose`], one more arm in `AccountMirrors::refresh`. Nothing else is touched.
//! What a Google source needs is in FINDINGS ("Lane graph-calendar").

mod adapter;
pub mod dav;
pub mod graph;

pub use adapter::FeedReplica;
pub use dav::{DavFeed, DavSource};
pub use graph::{GraphCalendarFeed, GraphCalendarSource};

use super::PimKind;
use super::discover::{DiscoverError, Found};
use porter_core::capability::{Capability, PimTransport};
use porter_core::{Candidate, ServiceEndpoint};
use porter_sync::{More, RemoteItem, ReplicaError, Tombstone};
use std::future::Future;

/// A position in a collection's change feed, in the source's own type.
pub trait FeedCursor: Sized + Send + Sync {
    /// The text the journal keeps as the anchor.
    fn encode(&self) -> String;
    /// The cursor the text stands for; `None` when it is not one of this source's (the feed is
    /// then read again from the start).
    fn decode(text: &str) -> Option<Self>;
}

/// One change of a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedChange {
    /// Created or changed. `content` is the item's bytes when the source already holds them
    /// (Graph's delta carries whole events); `None` when they are fetched afterwards (CalDAV's
    /// sync-collection reports names and etags only).
    Upsert {
        /// The item as the journal records it: id, version, name, size, hash.
        item: RemoteItem,
        /// Its iCalendar or vCard bytes, when known.
        content: Option<Vec<u8>>,
    },
    /// Deleted.
    Delete(Tombstone),
}

/// One page of a feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<C> {
    /// The changes, oldest first.
    pub changes: Vec<FeedChange>,
    /// Where the next read starts; stored once the page is applied.
    pub next: C,
    /// Whether more pages follow.
    pub more: More,
}

/// The changes of one collection.
pub trait Feed: Send + Sync + 'static {
    /// The source's cursor type.
    type Cursor: FeedCursor;

    /// Changes after `from`; `None` is the start: everything there is, without deletes. A cursor
    /// the source no longer honours is `ReplicaError::AnchorExpired`.
    fn changes(
        &self,
        from: Option<Self::Cursor>,
    ) -> impl Future<Output = Result<Page<Self::Cursor>, ReplicaError>> + Send;

    /// An item's bytes, for an upsert that came without them.
    fn fetch(
        &self,
        id: &porter_sync::RemoteId,
    ) -> impl Future<Output = Result<Vec<u8>, ReplicaError>> + Send;

    /// What the backend offers; the default is a read-only feed whose items the engine compares
    /// by SHA-256.
    fn features(&self) -> porter_core::capability::StorageCap {
        use porter_core::capability::{
            Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
        };
        StorageCap {
            access: Access::Read,
            delta: Delta::Poll,
            quota: QuotaReport::Unreported,
            scope: StorageScope::Full,
            hashes: HashKind::Sha256,
            ranges: Offered::Absent,
            chunked_upload: Offered::Absent,
        }
    }

    /// The quota, when `features` says it is reported.
    fn quota(&self) -> impl Future<Output = Result<porter_sync::Quota, ReplicaError>> + Send {
        async {
            Ok(porter_sync::Quota {
                used: porter_core::Bytes(0),
                total: None,
            })
        }
    }
}

/// The collections of an account's kind and the feed of each.
pub trait PimSource: Send + Sync + 'static {
    /// Its feeds.
    type Feed: Feed;

    /// The collections there are now: a name for the directory (`Found::segment`, stable for the
    /// collection), the URL that identifies it, its display name and colour.
    fn collections(
        &self,
        kind: PimKind,
    ) -> impl Future<Output = Result<Vec<Found>, DiscoverError>> + Send;

    /// The feed of one of those collections.
    fn feed(&self, found: &Found) -> Self::Feed;
}

/// Which source an account's grant for a kind is read through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chosen {
    /// CalDAV or CardDAV.
    Dav,
    /// Microsoft Graph calendars.
    GraphCalendar,
}

/// Why no source reads a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no source reads {kind:?} over {transport:?} yet")]
pub struct NoSource {
    /// The kind.
    pub kind: PimKind,
    /// The capability's transport.
    pub transport: PimTransport,
}

/// The capability's transport for `kind`.
fn transport_of(kind: PimKind, candidate: &Candidate) -> Option<PimTransport> {
    match (kind, &candidate.capability) {
        (PimKind::Calendar, Capability::Calendar(cap))
        | (PimKind::Contacts, Capability::Contacts(cap)) => Some(cap.transport),
        _ => None,
    }
}

/// The source for the account's capability: by its transport, never by provider name.
pub fn choose(kind: PimKind, candidate: &Candidate) -> Result<Chosen, NoSource> {
    // A candidate whose capability is not the kind's (or an older bus peer) is the kind's
    // protocol family, which is what the mirror did before sources existed.
    let transport = transport_of(kind, candidate).unwrap_or(match kind {
        PimKind::Calendar => PimTransport::CalDav,
        PimKind::Contacts => PimTransport::CardDav,
    });
    match (kind, transport) {
        (_, PimTransport::CalDav | PimTransport::CardDav) => Ok(Chosen::Dav),
        (PimKind::Calendar, PimTransport::Graph) => Ok(Chosen::GraphCalendar),
        (kind, transport) => Err(NoSource { kind, transport }),
    }
}

/// The account's server of `family`, else its first.
pub(crate) fn endpoint_of(
    candidate: &Candidate,
    family: porter_core::Family,
) -> Option<&ServiceEndpoint> {
    candidate
        .endpoints
        .iter()
        .find(|e| e.family == family)
        .or_else(|| candidate.endpoints.first())
}

#[cfg(test)]
mod tests;
