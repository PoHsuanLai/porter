//! The Google Calendar source: `GET users/me/calendarList` for the collections and, per
//! calendar, `GET calendars/{id}/events` for the changes, with `nextSyncToken` as the cursor,
//! `nextPageToken` paging and a cancelled event as a delete. `410` on a cursor is
//! `AnchorExpired`: the feed is read again from the start and nothing is deleted. A listing from
//! the start asks for no deleted events (there is nothing to delete yet); one from a sync token
//! must ask for them. Events are converted to iCalendar by [`super::event`] and arrive with
//! their bytes, so the engine's fetch costs no request.

use super::super::graph::name_of;
use super::super::{Feed, FeedChange, Page, PimSource};
use super::event::{self, Event};
use super::{Api, SyncCursor, id_in, list_all, read_error, segment_of};
use crate::dataset::fingerprint;
use crate::datasets::pim::PimKind;
use crate::datasets::pim::discover::{DiscoverError, Found};
use porter_client::{Accounts, Transport};
use porter_core::{Bytes, EndpointUrl, GrantId, WebUrl};
use porter_sync::{More, RemoteId, RemoteItem, RemoteVersion, ReplicaError, RetryAfter, Tombstone};
use serde::Deserialize;
use std::sync::Arc;
use storage_webdav::{Clock, DELETED};

/// How many events a page asks for.
const PAGE: &str = "250";

/// One entry of `calendarList`.
#[derive(Debug, Deserialize)]
struct CalendarEntry {
    id: String,
    summary: Option<String>,
    #[serde(rename = "summaryOverride")]
    summary_override: Option<String>,
    #[serde(rename = "backgroundColor")]
    background_color: Option<String>,
    #[serde(rename = "accessRole")]
    access_role: Option<String>,
}

/// One page of `events.list`.
#[derive(Debug, Deserialize)]
struct EventsPage {
    #[serde(default)]
    items: Vec<Event>,
    #[serde(rename = "nextPageToken")]
    next_page: Option<String>,
    #[serde(rename = "nextSyncToken")]
    next_sync: Option<String>,
}

/// The calendars of one Google account.
#[derive(Debug)]
pub struct GoogleCalendarSource<T: Transport> {
    api: Arc<Api<T>>,
}

impl<T: Transport> GoogleCalendarSource<T> {
    /// The source at `endpoint` (`https://www.googleapis.com/calendar/v3`), as `grant` allows.
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

/// One calendar's feed.
#[derive(Debug)]
pub struct GoogleCalendarFeed<T: Transport> {
    api: Arc<Api<T>>,
    calendar: String,
    clock: Clock,
}

impl<T: Transport + 'static> PimSource for GoogleCalendarSource<T> {
    type Feed = GoogleCalendarFeed<T>;

    async fn collections(&self, kind: PimKind) -> Result<Vec<Found>, DiscoverError> {
        debug_assert_eq!(kind, PimKind::Calendar, "this source reads calendars");
        let entries: Vec<CalendarEntry> = list_all(
            &self.api,
            "/users/me/calendarList",
            &[("maxResults", "250")],
        )
        .await?;
        entries
            .into_iter()
            // A calendar shared as free/busy only cannot be listed.
            .filter(|c| c.access_role.as_deref() != Some("freeBusyReader"))
            .map(|calendar| {
                let url = format!(
                    "{}/calendars/{}/",
                    self.api.base(),
                    super::super::graph::encode(&calendar.id)
                );
                Ok(Found {
                    url: WebUrl::parse(&url).map_err(|_| DiscoverError::Unreadable)?,
                    segment: segment_of(&calendar.id),
                    displayname: calendar
                        .summary_override
                        .or(calendar.summary)
                        .filter(|n| !n.is_empty()),
                    color: calendar.background_color.filter(|c| !c.is_empty()),
                })
            })
            .collect()
    }

    fn feed(&self, found: &Found) -> GoogleCalendarFeed<T> {
        GoogleCalendarFeed {
            api: Arc::clone(&self.api),
            calendar: id_in(&found.url),
            clock: Clock::system(),
        }
    }
}

impl<T: Transport> GoogleCalendarFeed<T> {
    fn events_path(&self, tail: &str) -> String {
        format!(
            "/calendars/{}/events{tail}",
            super::super::graph::encode(&self.calendar)
        )
    }

    /// An event as the journal records it, with the bytes it converts to; `None` for one that
    /// cannot be written (it is left out and said on standard error).
    fn upsert(&self, event: &Event) -> Option<FeedChange> {
        let ics = match event::to_ics(event) {
            Ok(ics) => ics.into_bytes(),
            Err(why) => {
                eprintln!("syncd: a Google calendar event is not mirrored: {why}");
                return None;
            }
        };
        let version = event
            .etag
            .as_deref()
            .or(event.updated.as_deref())
            .map_or_else(|| fingerprint(&ics).0, str::to_owned);
        Some(FeedChange::Upsert {
            item: RemoteItem {
                id: RemoteId(event.id.clone()),
                version: RemoteVersion(version),
                path: name_of(&event.id),
                size: Bytes(ics.len() as u64),
                hash: Some(fingerprint(&ics)),
            },
            content: Some(ics),
        })
    }
}

impl<T: Transport + 'static> Feed for GoogleCalendarFeed<T> {
    type Cursor = SyncCursor;

    async fn changes(&self, from: Option<SyncCursor>) -> Result<Page<SyncCursor>, ReplicaError> {
        let resuming = from.is_some();
        let (sync, page) = match from {
            None => (None, None),
            Some(SyncCursor::Synced(token)) => (Some(token), None),
            Some(SyncCursor::Paging { page, sync }) => (sync, Some(page)),
        };
        let mut query = vec![("maxResults", PAGE)];
        if let Some(token) = sync.as_deref() {
            query.push(("syncToken", token));
            query.push(("showDeleted", "true"));
        }
        if let Some(token) = page.as_deref() {
            query.push(("pageToken", token));
        }
        let response = self
            .api
            .read(&self.api.url(&self.events_path(""), &query))
            .await?;
        match (response.status.0, resuming) {
            (200, _) => {}
            // `fullSyncRequired`: the token (or the page it began with) is no longer honoured.
            (410, true) => return Err(ReplicaError::AnchorExpired),
            _ => return Err(read_error(&response)),
        }
        let listing: EventsPage = serde_json::from_slice(&response.body)
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        let (next, more) = match (listing.next_page, listing.next_sync) {
            (Some(page), _) if !page.is_empty() => (SyncCursor::Paging { page, sync }, More::More),
            (_, Some(token)) if !token.is_empty() => (SyncCursor::Synced(token), More::Done),
            _ => return Err(ReplicaError::Transient(RetryAfter(30))),
        };
        let changes = listing
            .items
            .iter()
            .filter_map(|event| match event.is_cancelled() {
                true => Some(FeedChange::Delete(Tombstone {
                    id: RemoteId(event.id.clone()),
                    version: RemoteVersion(DELETED.to_owned()),
                    deleted_at: self.clock.now(),
                })),
                false => self.upsert(event),
            })
            .collect();
        Ok(Page {
            changes,
            next,
            more,
        })
    }

    async fn fetch(&self, id: &RemoteId) -> Result<Vec<u8>, ReplicaError> {
        let tail = format!("/{}", super::super::graph::encode(&id.0));
        let response = self
            .api
            .read(&self.api.url(&self.events_path(&tail), &[]))
            .await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let event: Event = serde_json::from_slice(&response.body)
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        match event.is_cancelled() {
            true => Err(ReplicaError::Gone),
            false => event::to_ics(&event)
                .map(String::into_bytes)
                .map_err(|_| ReplicaError::Gone),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::pim::source::FeedCursor;

    #[test]
    fn a_calendar_entry_is_named_by_the_override_else_the_summary() {
        let entry: CalendarEntry = serde_json::from_str(
            r##"{"id": "a@b", "summary": "Real", "summaryOverride": "Mine", "backgroundColor": "#9fe1e7"}"##,
        )
        .expect("entry");
        assert_eq!(
            entry.summary_override.or(entry.summary).as_deref(),
            Some("Mine")
        );
        assert_eq!(entry.background_color.as_deref(), Some("#9fe1e7"));
    }

    #[test]
    fn the_cursor_a_page_leaves_keeps_the_listings_sync_token() {
        let cursor = SyncCursor::Paging {
            page: "p2".into(),
            sync: Some("s1".into()),
        };
        assert_eq!(cursor.encode(), "page:p2:s1");
    }
}
