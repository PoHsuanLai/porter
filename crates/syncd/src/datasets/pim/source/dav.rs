//! The CalDAV and CardDAV source: today's behaviour behind the seam. Collections come from the
//! principal and home sets ([`super::super::discover`]), each collection's feed is porter-dav's
//! sync-collection (or the etag walk) through a [`WebDavReplica`] over the account's relay; its
//! upserts carry no content, so the engine fetches each item, as before.

use super::super::PimKind;
use super::super::discover::{DiscoverError, Found, discover};
use super::super::relay::{PimDial, pim_http, pim_replica};
use super::{Feed, FeedChange, FeedCursor, Page, PimSource};
use porter_client::{Accounts, Transport};
use porter_core::{EndpointUrl, GrantId, WebUrl};
use porter_sync::{Anchor, ByteRange, Change, Cursor, RemoteId, Replica, ReplicaError};
use std::sync::Arc;
use storage_webdav::{Clock, StreamHttp, WebDavReplica};

/// A WebDAV sync token or etag-walk position: the replica's own anchor text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavCursor(pub String);

impl FeedCursor for DavCursor {
    fn encode(&self) -> String {
        self.0.clone()
    }

    fn decode(text: &str) -> Option<Self> {
        Some(Self(text.to_owned()))
    }
}

/// The collections of one account's CalDAV or CardDAV endpoint.
#[derive(Debug)]
pub struct DavSource<T> {
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
}

impl<T> DavSource<T> {
    /// The source at `endpoint`, as `grant` allows.
    pub fn new(accounts: Arc<Accounts<T>>, grant: GrantId, endpoint: EndpointUrl) -> Self {
        Self {
            accounts,
            grant,
            endpoint,
        }
    }
}

/// One DAV collection's feed.
#[derive(Debug)]
pub struct DavFeed<T: Transport>(WebDavReplica<StreamHttp<PimDial<T>>>);

impl<T: Transport + 'static> PimSource for DavSource<T> {
    type Feed = DavFeed<T>;

    async fn collections(&self, kind: PimKind) -> Result<Vec<Found>, DiscoverError> {
        let web = WebUrl::try_from(&self.endpoint).map_err(|_| DiscoverError::Unreadable)?;
        let http = pim_http(
            Arc::clone(&self.accounts),
            self.grant.clone(),
            self.endpoint.clone(),
        );
        discover(&http, &web, kind).await
    }

    fn feed(&self, found: &Found) -> DavFeed<T> {
        DavFeed(pim_replica(
            Arc::clone(&self.accounts),
            self.grant.clone(),
            self.endpoint.clone(),
            &found.url,
            Clock::system(),
        ))
    }
}

impl<T: Transport + 'static> Feed for DavFeed<T> {
    type Cursor = DavCursor;

    async fn changes(&self, from: Option<DavCursor>) -> Result<Page<DavCursor>, ReplicaError> {
        let cursor = from.map_or(Cursor::Start, |c| Cursor::At(Anchor(c.0)));
        let page = self.0.changes(cursor).await?;
        Ok(Page {
            changes: page
                .changes
                .into_iter()
                .map(|change| match change {
                    Change::Upsert(item) => FeedChange::Upsert {
                        item,
                        content: None,
                    },
                    Change::Tombstone(tombstone) => FeedChange::Delete(tombstone),
                })
                .collect(),
            next: DavCursor(page.next.0),
            more: page.more,
        })
    }

    async fn fetch(&self, id: &RemoteId) -> Result<Vec<u8>, ReplicaError> {
        Ok(self.0.fetch(id, ByteRange::Whole).await?.0)
    }

    fn features(&self) -> porter_core::capability::StorageCap {
        self.0.features()
    }

    async fn quota(&self) -> Result<porter_sync::Quota, ReplicaError> {
        self.0.quota().await
    }
}
