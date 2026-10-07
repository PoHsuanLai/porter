//! The cursors of the Google feeds: where a read of a collection resumes.
//!
//! Calendar and People hand out a sync token when a listing is finished and a page token while
//! it is not; a page token only works with the parameters the listing began with, so a cursor
//! in the middle of a listing keeps the sync token it began with. Tasks has no token: the cursor
//! is the newest `updated` time seen, and a cursor in the middle of a listing keeps the time the
//! listing began after and the newest seen so far. Values are percent-encoded so the text splits
//! on `:`.

use super::super::FeedCursor;
use super::super::graph::{decode, encode};

/// A position in an `events.list` or `people.connections.list` read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncCursor {
    /// The listing finished and handed out this sync token.
    Synced(String),
    /// The listing is on a later page.
    Paging {
        /// The token of the page to read next.
        page: String,
        /// The sync token the listing began with, when it was incremental.
        sync: Option<String>,
    },
}

impl FeedCursor for SyncCursor {
    fn encode(&self) -> String {
        match self {
            SyncCursor::Synced(token) => format!("sync:{}", encode(token)),
            SyncCursor::Paging { page, sync } => format!(
                "page:{}:{}",
                encode(page),
                sync.as_deref().map(encode).unwrap_or_default()
            ),
        }
    }

    fn decode(text: &str) -> Option<Self> {
        let (tag, rest) = text.split_once(':')?;
        match tag {
            "sync" => (!rest.is_empty()).then_some(SyncCursor::Synced(decode(rest))),
            "page" => {
                let (page, sync) = rest.split_once(':')?;
                (!page.is_empty()).then(|| SyncCursor::Paging {
                    page: decode(page),
                    sync: Some(sync).filter(|s| !s.is_empty()).map(decode),
                })
            }
            _ => None,
        }
    }
}

/// A position in a `tasks.list` read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCursor {
    /// Everything updated at or after this time has been read; `None` is nothing yet.
    Since(Option<String>),
    /// The listing is on a later page.
    Paging {
        /// The token of the page to read next.
        page: String,
        /// The time the listing asks updates since.
        since: Option<String>,
        /// The newest `updated` seen on the pages so far.
        newest: Option<String>,
    },
}

impl TaskCursor {
    /// The time a listing from here asks updates since.
    pub fn since(&self) -> Option<&str> {
        match self {
            TaskCursor::Since(since) | TaskCursor::Paging { since, .. } => since.as_deref(),
        }
    }
}

fn optional(text: &str) -> Option<String> {
    Some(text).filter(|t| !t.is_empty()).map(decode)
}

impl FeedCursor for TaskCursor {
    fn encode(&self) -> String {
        let text = |value: &Option<String>| value.as_deref().map(encode).unwrap_or_default();
        match self {
            TaskCursor::Since(since) => format!("since:{}", text(since)),
            TaskCursor::Paging {
                page,
                since,
                newest,
            } => format!("page:{}:{}:{}", encode(page), text(since), text(newest)),
        }
    }

    fn decode(text: &str) -> Option<Self> {
        let (tag, rest) = text.split_once(':')?;
        match tag {
            "since" => Some(TaskCursor::Since(optional(rest))),
            "page" => {
                let mut parts = rest.splitn(3, ':');
                let (page, since, newest) = (parts.next()?, parts.next()?, parts.next()?);
                (!page.is_empty()).then(|| TaskCursor::Paging {
                    page: decode(page),
                    since: optional(since),
                    newest: optional(newest),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sync_cursor_round_trips_whatever_the_tokens_hold() {
        for cursor in [
            SyncCursor::Synced("CJDCkdHaxfICEJDC=".into()),
            SyncCursor::Synced("with:colon and space/slash%".into()),
            SyncCursor::Paging {
                page: "EAE".into(),
                sync: None,
            },
            SyncCursor::Paging {
                page: "a:b".into(),
                sync: Some("s:t".into()),
            },
        ] {
            let text = cursor.encode();
            assert_eq!(SyncCursor::decode(&text), Some(cursor), "{text}");
        }
    }

    #[test]
    fn a_task_cursor_round_trips_whatever_the_times_hold() {
        for cursor in [
            TaskCursor::Since(None),
            TaskCursor::Since(Some("2026-10-08T07:21:03.000Z".into())),
            TaskCursor::Paging {
                page: "tok".into(),
                since: None,
                newest: None,
            },
            TaskCursor::Paging {
                page: "t:k".into(),
                since: Some("2026-10-08T07:21:03.000Z".into()),
                newest: Some("2026-10-09T00:00:00.000Z".into()),
            },
        ] {
            let text = cursor.encode();
            assert_eq!(TaskCursor::decode(&text), Some(cursor), "{text}");
        }
    }

    #[test]
    fn text_that_is_not_a_cursor_of_the_source_is_not_one() {
        for text in [
            "",
            "sync",
            "sync:",
            "page:",
            "page:x",
            "token",
            "https://graph.test/delta?x=1",
            "since:2026",
        ] {
            assert_eq!(SyncCursor::decode(text), None, "{text:?}");
        }
        for text in [
            "", "sync:abc", "page:", "page:x", "page:x:y", "x:y", "since",
        ] {
            assert_eq!(TaskCursor::decode(text), None, "{text:?}");
        }
    }
}
