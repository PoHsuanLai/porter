//! The Google contacts source (People API): one collection, "Contacts", because the API has no
//! address books; `GET people/me/connections` with `requestSyncToken=true` for the changes, the
//! `nextSyncToken` as the cursor, `nextPageToken` paging and `metadata.deleted` as a delete. A
//! sync token past its life is `400` with `EXPIRED_SYNC_TOKEN`, which is `AnchorExpired`.
//! Contacts are converted to vCard 3.0 by [`super::person`] and arrive with their bytes.

use super::super::graph::{encode, name_of};
use super::super::{Feed, FeedChange, Page, PimSource};
use super::person::{self, Person};
use super::{Api, SyncCursor, read_error};
use crate::dataset::fingerprint;
use crate::datasets::pim::PimKind;
use crate::datasets::pim::discover::{DiscoverError, Found};
use porter_client::{Accounts, Transport};
use porter_core::{Bytes, EndpointUrl, GrantId, WebUrl};
use porter_sync::{More, RemoteId, RemoteItem, RemoteVersion, ReplicaError, RetryAfter, Tombstone};
use serde::Deserialize;
use std::sync::Arc;
use storage_webdav::{Clock, DELETED};

/// The fields asked for: what the converter carries, and the metadata that says "deleted".
const FIELDS: &str =
    "names,emailAddresses,phoneNumbers,addresses,organizations,birthdays,biographies,urls,metadata";

/// How many contacts a page asks for.
const PAGE: &str = "200";

/// The name of the one collection.
const NAME: &str = "Contacts";

/// One page of `people.connections.list`.
#[derive(Debug, Deserialize)]
struct Connections {
    #[serde(default)]
    connections: Vec<Person>,
    #[serde(rename = "nextPageToken")]
    next_page: Option<String>,
    #[serde(rename = "nextSyncToken")]
    next_sync: Option<String>,
}

/// The contacts of one Google account.
#[derive(Debug)]
pub struct GooglePeopleSource<T: Transport> {
    api: Arc<Api<T>>,
}

impl<T: Transport> GooglePeopleSource<T> {
    /// The source at `endpoint` (`https://people.googleapis.com/v1`), as `grant` allows.
    pub fn new(
        accounts: Arc<Accounts<T>>,
        grant: GrantId,
        endpoint: EndpointUrl,
    ) -> Result<Self, DiscoverError> {
        Ok(Self {
            api: Arc::new(Api::new(accounts, grant, endpoint)?),
        })
    }
}

/// The contacts' feed.
#[derive(Debug)]
pub struct GooglePeopleFeed<T: Transport> {
    api: Arc<Api<T>>,
    clock: Clock,
}

impl<T: Transport + 'static> PimSource for GooglePeopleSource<T> {
    type Feed = GooglePeopleFeed<T>;

    async fn collections(&self, kind: PimKind) -> Result<Vec<Found>, DiscoverError> {
        debug_assert_eq!(kind, PimKind::Contacts, "this source reads contacts");
        let url = format!("{}/people/me/connections/", self.api.base());
        Ok(vec![Found {
            url: WebUrl::parse(&url).map_err(|_| DiscoverError::Unreadable)?,
            segment: "google".to_owned(),
            displayname: Some(NAME.to_owned()),
            color: None,
        }])
    }

    fn feed(&self, _found: &Found) -> GooglePeopleFeed<T> {
        GooglePeopleFeed {
            api: Arc::clone(&self.api),
            clock: Clock::system(),
        }
    }
}

/// The file name of a contact: its resource name with the slash made safe.
fn file_of(person: &Person) -> porter_sync::ItemPath {
    let stem = person.resource_name.replace('/', "_");
    match stem.is_empty() {
        true => name_of(&person.resource_name),
        false => porter_sync::ItemPath(format!("{stem}.vcf")),
    }
}

impl<T: Transport> GooglePeopleFeed<T> {
    fn upsert(person: &Person) -> FeedChange {
        let vcf = person::to_vcf(person).into_bytes();
        let version = person.etag.clone().unwrap_or_else(|| fingerprint(&vcf).0);
        FeedChange::Upsert {
            item: RemoteItem {
                id: RemoteId(person.resource_name.clone()),
                version: RemoteVersion(version),
                path: file_of(person),
                size: Bytes(vcf.len() as u64),
                hash: Some(fingerprint(&vcf)),
            },
            content: Some(vcf),
        }
    }
}

impl<T: Transport + 'static> Feed for GooglePeopleFeed<T> {
    type Cursor = SyncCursor;

    async fn changes(&self, from: Option<SyncCursor>) -> Result<Page<SyncCursor>, ReplicaError> {
        let resuming = from.is_some();
        let (sync, page) = match from {
            None => (None, None),
            Some(SyncCursor::Synced(token)) => (Some(token), None),
            Some(SyncCursor::Paging { page, sync }) => (sync, Some(page)),
        };
        let mut query = vec![
            ("personFields", FIELDS),
            ("pageSize", PAGE),
            ("requestSyncToken", "true"),
        ];
        if let Some(token) = sync.as_deref() {
            query.push(("syncToken", token));
        }
        if let Some(token) = page.as_deref() {
            query.push(("pageToken", token));
        }
        let response = self
            .api
            .read(&self.api.url("/people/me/connections", &query))
            .await?;
        match (response.status.0, resuming) {
            (200, _) => {}
            (400, true)
                if String::from_utf8_lossy(&response.body).contains("EXPIRED_SYNC_TOKEN") =>
            {
                return Err(ReplicaError::AnchorExpired);
            }
            _ => return Err(read_error(&response)),
        }
        let listing: Connections = serde_json::from_slice(&response.body)
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        let (next, more) = match (listing.next_page, listing.next_sync) {
            (Some(page), _) if !page.is_empty() => (SyncCursor::Paging { page, sync }, More::More),
            (_, Some(token)) if !token.is_empty() => (SyncCursor::Synced(token), More::Done),
            _ => return Err(ReplicaError::Transient(RetryAfter(30))),
        };
        let changes = listing
            .connections
            .iter()
            .map(|person| match person.is_deleted() {
                true => FeedChange::Delete(Tombstone {
                    id: RemoteId(person.resource_name.clone()),
                    version: RemoteVersion(DELETED.to_owned()),
                    deleted_at: self.clock.now(),
                }),
                false => Self::upsert(person),
            })
            .collect();
        Ok(Page {
            changes,
            next,
            more,
        })
    }

    async fn fetch(&self, id: &RemoteId) -> Result<Vec<u8>, ReplicaError> {
        // The resource name is `people/c123`: its slash is the path's.
        let path = format!(
            "/{}",
            id.0.split('/').map(encode).collect::<Vec<_>>().join("/")
        );
        let response = self
            .api
            .read(&self.api.url(&path, &[("personFields", FIELDS)]))
            .await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let person: Person = serde_json::from_slice(&response.body)
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        match person.is_deleted() {
            true => Err(ReplicaError::Gone),
            false => Ok(person::to_vcf(&person).into_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contacts_file_is_its_resource_name_made_safe() {
        let person: Person =
            serde_json::from_str(r#"{"resourceName": "people/c1001"}"#).expect("person");
        assert_eq!(file_of(&person).0, "people_c1001.vcf");
    }
}
