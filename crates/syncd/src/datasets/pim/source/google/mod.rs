//! The Google sources: Calendar, People (contacts) and Tasks, each a [`PimSource`] and a
//! [`Feed`] behind the seam, all through the relay accountd opens
//! (`Tokens.OpenAuthenticated`) to the capability's endpoint, so syncd never holds the Google
//! token. One module per API, each with a pure converter to the vdir's formats:
//!
//! | API | collections | changes | converter |
//! |---|---|---|---|
//! | Calendar v3 | `users/me/calendarList` | `events.list` with `syncToken`; `410` is expired | [`event`]: JSON to `VEVENT` |
//! | People v1 | one, "Contacts" | `people.connections.list` with `requestSyncToken`; `EXPIRED_SYNC_TOKEN` | [`person`]: JSON to vCard 3.0 |
//! | Tasks v1 | `users/@me/lists` | `tasks.list` with `updatedMin` and `showDeleted` | [`task`]: JSON to `VTODO` |
//!
//! **Unverified against Google** (no network in this build; the paths, parameters and error
//! shapes are from Google's published reference as remembered, and the fake serves exactly
//! these; check them when the owner's client first runs): the endpoint paths and query
//! parameters named above; that `events.list` with a `syncToken` returns cancelled stubs
//! whatever `showDeleted` says and rejects other filters; that an invalid sync token is `410`
//! with reason `fullSyncRequired`; that a People sync token past its life answers `400` with
//! `EXPIRED_SYNC_TOKEN` in the body; that `updatedMin` is inclusive (the feed copes with either)
//! and that a deleted task stays listable with `showDeleted=true` and `deleted: true`; that
//! `pageSize` up to 1000 and `maxResults` up to 100 (Tasks) and 2500 (Events) are honoured.

mod calendar;
pub mod event;
mod people;
pub mod person;
mod sync;
pub mod task;
mod tasks;
mod time;

pub use calendar::{GoogleCalendarFeed, GoogleCalendarSource};
pub use people::{GooglePeopleFeed, GooglePeopleSource};
pub use sync::{SyncCursor, TaskCursor};
pub use tasks::{GoogleTasksFeed, GoogleTasksSource};

use super::graph::{encode, get, retry_after, unreached};
use crate::datasets::pim::discover::DiscoverError;
use crate::datasets::pim::relay::{PimDial, pim_http};
use porter_client::{Accounts, Transport};
use porter_core::{EndpointUrl, GrantId, WebUrl};
use porter_http::{HttpError, HttpResponse};
use porter_sync::ReplicaError;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use storage_webdav::StreamHttp;

/// The pages a listing of collections is followed for before a looping server is given up on.
const MAX_PAGES: usize = 50;

/// One Google API at one endpoint, reached through the relay.
#[derive(Debug)]
pub(super) struct Api<T: Transport> {
    http: StreamHttp<PimDial<T>>,
    base: String,
}

impl<T: Transport> Api<T> {
    /// The API at `endpoint` (its version path included), as `grant` allows.
    pub(super) fn new(
        accounts: Arc<Accounts<T>>,
        grant: GrantId,
        endpoint: EndpointUrl,
    ) -> Result<Self, DiscoverError> {
        let base = WebUrl::try_from(&endpoint).map_err(|_| DiscoverError::Unreadable)?;
        Ok(Self {
            base: base.as_str().trim_end_matches('/').to_owned(),
            http: pim_http(accounts, grant, endpoint),
        })
    }

    /// `base` + `path` + the query, each value percent-encoded.
    pub(super) fn url(&self, path: &str, query: &[(&str, &str)]) -> String {
        let query: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", encode(value)))
            .collect();
        match query.is_empty() {
            true => format!("{}{path}", self.base),
            false => format!("{}{path}?{}", self.base, query.join("&")),
        }
    }

    /// The API's base URL, no trailing slash.
    pub(super) fn base(&self) -> &str {
        &self.base
    }

    pub(super) async fn get(&self, url: &str) -> Result<HttpResponse, HttpError> {
        get(&self.http, url, None).await
    }

    /// A GET whose failure is a replica's: unreachable is transient.
    pub(super) async fn read(&self, url: &str) -> Result<HttpResponse, ReplicaError> {
        self.get(url).await.map_err(unreached)
    }
}

/// One page of a Google list: the items and the token of the next page.
#[derive(Debug, Deserialize)]
struct Listing<I> {
    #[serde(default = "Vec::new")]
    items: Vec<I>,
    #[serde(rename = "nextPageToken")]
    next: Option<String>,
}

/// Whether an error body says the caller is going too fast (Google answers some of these `403`).
pub(super) fn rate_limited(response: &HttpResponse) -> bool {
    let body = String::from_utf8_lossy(&response.body);
    [
        "rateLimitExceeded",
        "userRateLimitExceeded",
        "quotaExceeded",
    ]
    .iter()
    .any(|reason| body.contains(reason))
}

/// What a failed read means to a replica.
pub(super) fn read_error(response: &HttpResponse) -> ReplicaError {
    match response.status.0 {
        403 if rate_limited(response) => ReplicaError::Transient(retry_after(response)),
        401 | 403 => ReplicaError::Unauthorized,
        404 => ReplicaError::Gone,
        _ => ReplicaError::Transient(retry_after(response)),
    }
}

/// Every item of a paged list (`items` and `nextPageToken`) at `path`.
pub(super) async fn list_all<T: Transport, I: DeserializeOwned>(
    api: &Api<T>,
    path: &str,
    query: &[(&str, &str)],
) -> Result<Vec<I>, DiscoverError> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut asked: Vec<(&str, &str)> = query.to_vec();
        if let Some(page) = token.as_deref() {
            asked.push(("pageToken", page));
        }
        let response = api.get(&api.url(path, &asked)).await.map_err(|e| match e {
            HttpError::Unreachable | HttpError::TimedOut => DiscoverError::Unreachable,
            _ => DiscoverError::Unreadable,
        })?;
        match response.status.0 {
            200 => {}
            403 if rate_limited(&response) => return Err(DiscoverError::Status(429)),
            401 | 403 => return Err(DiscoverError::Unauthorized),
            other => return Err(DiscoverError::Status(other)),
        }
        let page: Listing<I> =
            serde_json::from_slice(&response.body).map_err(|_| DiscoverError::Unreadable)?;
        all.extend(page.items);
        match page.next.filter(|next| !next.is_empty()) {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    Ok(all)
}

/// The directory name of a collection: stable under renames and short (Google's calendar ids are
/// addresses and can be long).
pub(super) fn segment_of(id: &str) -> String {
    format!(
        "google-{}",
        super::graph::hex(&Sha256::digest(id.as_bytes())[..6])
    )
}

/// The id a collection's URL (`.../calendars/<id>/`) was made from.
pub(super) fn id_in(url: &WebUrl) -> String {
    url.path()
        .split('/')
        .rfind(|part| !part.is_empty())
        .map(super::graph::decode)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::{Header, Status};

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status: Status(status),
            headers: vec![Header::new("Retry-After", "7")],
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn a_failed_read_means_what_google_meant() {
        use ReplicaError as E;
        let limited = r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#;
        let denied = r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#;
        let cases = [
            (401, "", E::Unauthorized),
            (403, denied, E::Unauthorized),
            (403, limited, E::Transient(porter_sync::RetryAfter(7))),
            (404, "", E::Gone),
            (429, "", E::Transient(porter_sync::RetryAfter(7))),
            (503, "", E::Transient(porter_sync::RetryAfter(7))),
        ];
        for (status, body, want) in cases {
            assert_eq!(read_error(&response(status, body)), want, "{status}");
        }
    }

    #[test]
    fn a_collection_directory_is_short_stable_and_made_of_safe_characters() {
        let id = "en.usa#holiday@group.v.calendar.google.com";
        assert_eq!(segment_of(id), segment_of(id));
        assert_ne!(segment_of(id), segment_of("ada@gmail.com"));
        assert!(segment_of(id).len() < 20);
        assert!(
            segment_of(id)
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        );
    }

    #[test]
    fn a_collection_url_gives_back_the_id_it_was_made_from() {
        for id in [
            "ada@gmail.com",
            "en.usa#holiday@group.v.calendar.google.com",
            "MTIzNDU2",
        ] {
            let url = WebUrl::parse(&format!("https://x.test/v3/calendars/{}/", encode(id)))
                .expect("url");
            assert_eq!(id_in(&url), id);
        }
    }
}
