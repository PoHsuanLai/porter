//! The Google Tasks source: `GET users/@me/lists` for the collections and, per list,
//! `GET lists/{id}/tasks` for the changes. Tasks has no sync token, so the cursor is the newest
//! `updated` time seen (`updatedMin` on the next read, which may repeat the newest task: the
//! engine sees the same version and does nothing) and a deleted task, which `showDeleted` lists
//! with `deleted: true`, is a delete. A read from the start asks for no deleted tasks and does
//! ask for hidden (completed and cleared) ones. Tasks are converted to `VTODO`s by
//! [`super::task`] and arrive with their bytes.

use super::super::graph::encode;
use super::super::{Feed, FeedChange, Page, PimSource};
use super::task::{self, Task, TaskList};
use super::{Api, TaskCursor, id_in, list_all, read_error, segment_of};
use crate::dataset::fingerprint;
use crate::datasets::pim::PimKind;
use crate::datasets::pim::discover::{DiscoverError, Found};
use porter_client::{Accounts, Transport};
use porter_core::{Bytes, EndpointUrl, GrantId, WebUrl};
use porter_sync::{
    ItemPath, More, RemoteId, RemoteItem, RemoteVersion, ReplicaError, RetryAfter, Tombstone,
};
use serde::Deserialize;
use std::sync::Arc;
use storage_webdav::{Clock, DELETED};

/// How many tasks a page asks for (Google's most).
const PAGE: &str = "100";

/// One page of `tasks.list`.
#[derive(Debug, Deserialize)]
struct TasksPage {
    #[serde(default)]
    items: Vec<Task>,
    #[serde(rename = "nextPageToken")]
    next_page: Option<String>,
}

/// The task lists of one Google account.
#[derive(Debug)]
pub struct GoogleTasksSource<T: Transport> {
    api: Arc<Api<T>>,
}

impl<T: Transport> GoogleTasksSource<T> {
    /// The source at `endpoint` (`https://tasks.googleapis.com/tasks/v1`), as `grant` allows.
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

/// One task list's feed.
#[derive(Debug)]
pub struct GoogleTasksFeed<T: Transport> {
    api: Arc<Api<T>>,
    list: String,
    clock: Clock,
}

impl<T: Transport + 'static> PimSource for GoogleTasksSource<T> {
    type Feed = GoogleTasksFeed<T>;

    async fn collections(&self, kind: PimKind) -> Result<Vec<Found>, DiscoverError> {
        debug_assert_eq!(kind, PimKind::Tasks, "this source reads task lists");
        let lists: Vec<TaskList> =
            list_all(&self.api, "/users/@me/lists", &[("maxResults", "100")]).await?;
        lists
            .into_iter()
            .map(|list| {
                let url = format!("{}/lists/{}/", self.api.base(), encode(&list.id));
                Ok(Found {
                    url: WebUrl::parse(&url).map_err(|_| DiscoverError::Unreadable)?,
                    segment: segment_of(&list.id),
                    displayname: list.title.filter(|t| !t.is_empty()),
                    color: None,
                })
            })
            .collect()
    }

    fn feed(&self, found: &Found) -> GoogleTasksFeed<T> {
        GoogleTasksFeed {
            api: Arc::clone(&self.api),
            list: id_in(&found.url),
            clock: Clock::system(),
        }
    }
}

/// The file name of a task.
fn file_of(task: &Task) -> ItemPath {
    ItemPath(format!("{}.ics", task.id))
}

impl<T: Transport> GoogleTasksFeed<T> {
    fn tasks_path(&self, tail: &str) -> String {
        format!("/lists/{}/tasks{tail}", encode(&self.list))
    }

    fn upsert(task: &Task) -> FeedChange {
        let ics = task::to_ics(task).into_bytes();
        let version = task
            .etag
            .as_deref()
            .or(task.updated.as_deref())
            .map_or_else(|| fingerprint(&ics).0, str::to_owned);
        FeedChange::Upsert {
            item: RemoteItem {
                id: RemoteId(task.id.clone()),
                version: RemoteVersion(version),
                path: file_of(task),
                size: Bytes(ics.len() as u64),
                hash: Some(fingerprint(&ics)),
            },
            content: Some(ics),
        }
    }
}

impl<T: Transport + 'static> Feed for GoogleTasksFeed<T> {
    type Cursor = TaskCursor;

    async fn changes(&self, from: Option<TaskCursor>) -> Result<Page<TaskCursor>, ReplicaError> {
        let (since, page, newest) = match from {
            None => (None, None, None),
            Some(TaskCursor::Since(since)) => (since, None, None),
            Some(TaskCursor::Paging {
                page,
                since,
                newest,
            }) => (since, Some(page), newest),
        };
        let mut query = vec![
            ("maxResults", PAGE),
            ("showCompleted", "true"),
            ("showHidden", "true"),
        ];
        if let Some(since) = since.as_deref() {
            query.push(("updatedMin", since));
            query.push(("showDeleted", "true"));
        }
        if let Some(token) = page.as_deref() {
            query.push(("pageToken", token));
        }
        let response = self
            .api
            .read(&self.api.url(&self.tasks_path(""), &query))
            .await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let listing: TasksPage = serde_json::from_slice(&response.body)
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        // `updated` is always `YYYY-MM-DDTHH:MM:SS.sssZ`, which orders as text.
        let newest = listing
            .items
            .iter()
            .filter_map(|t| t.updated.clone())
            .chain(newest)
            .max();
        let (next, more) = match listing.next_page.filter(|p| !p.is_empty()) {
            Some(page) => (
                TaskCursor::Paging {
                    page,
                    since,
                    newest,
                },
                More::More,
            ),
            None => (TaskCursor::Since(newest.or(since)), More::Done),
        };
        let changes = listing
            .items
            .iter()
            .map(|task| match task.is_deleted() {
                true => FeedChange::Delete(Tombstone {
                    id: RemoteId(task.id.clone()),
                    version: RemoteVersion(DELETED.to_owned()),
                    deleted_at: self.clock.now(),
                }),
                false => Self::upsert(task),
            })
            .collect();
        Ok(Page {
            changes,
            next,
            more,
        })
    }

    async fn fetch(&self, id: &RemoteId) -> Result<Vec<u8>, ReplicaError> {
        let tail = format!("/{}", encode(&id.0));
        let response = self
            .api
            .read(&self.api.url(&self.tasks_path(&tail), &[]))
            .await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let task: Task = serde_json::from_slice(&response.body)
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        match task.is_deleted() {
            true => Err(ReplicaError::Gone),
            false => Ok(task::to_ics(&task).into_bytes()),
        }
    }
}
