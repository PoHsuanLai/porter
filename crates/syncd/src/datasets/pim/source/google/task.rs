//! A Google Tasks task as an iCalendar `VTODO`: pure, no I/O.
//!
//! | Google | iCalendar |
//! |---|---|
//! | `id` | `UID` |
//! | `title`, `notes` | `SUMMARY`, `DESCRIPTION` |
//! | `due` (a date; Google keeps no time of day) | `DUE;VALUE=DATE` |
//! | `status` `needsAction`, `completed` | `STATUS:NEEDS-ACTION`, `STATUS:COMPLETED` |
//! | `completed` (a time) | `COMPLETED` (UTC) |
//! | `parent` | `RELATED-TO;RELTYPE=PARENT` |
//! | `updated` | `DTSTAMP` and `LAST-MODIFIED` |
//!
//! Not carried: `position` (the order in the list), `links`, `hidden`. A list is a directory and
//! each of its tasks a file, so the list's name is the directory's `displayname`.

use super::super::graph::convert::{escape, fold};
use super::time::{Stamp, compact_date};
use serde::Deserialize;

/// A task list, as `tasklists.list` gives it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TaskList {
    /// Its id.
    pub id: String,
    /// Its name.
    pub title: Option<String>,
}

/// A task, as `tasks.list` and `tasks.get` give it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Task {
    /// Its id.
    pub id: String,
    /// Its version.
    pub etag: Option<String>,
    /// The title.
    pub title: Option<String>,
    /// The notes.
    pub notes: Option<String>,
    /// `needsAction` or `completed`.
    pub status: Option<String>,
    /// The due date, as an RFC 3339 time at midnight UTC.
    pub due: Option<String>,
    /// When it was completed.
    pub completed: Option<String>,
    /// The task it is a subtask of.
    pub parent: Option<String>,
    /// When it last changed.
    pub updated: Option<String>,
    /// Set on a deleted task, which `showDeleted` lists.
    pub deleted: Option<bool>,
}

impl Task {
    /// Whether the task was deleted: the item is gone.
    pub fn is_deleted(&self) -> bool {
        self.deleted == Some(true)
    }
}

/// The `VTODO` of `task`, in a `VCALENDAR`.
pub fn to_ics(task: &Task) -> String {
    let updated = task
        .updated
        .as_deref()
        .and_then(Stamp::parse)
        .map(|s| s.utc());
    let mut lines: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "VERSION:2.0".into(),
        "PRODID:-//porter//syncd google tasks//EN".into(),
        "BEGIN:VTODO".into(),
        format!("UID:{}", escape(&task.id)),
        format!(
            "DTSTAMP:{}",
            updated.as_deref().unwrap_or("19700101T000000Z")
        ),
    ];
    lines.extend(updated.map(|u| format!("LAST-MODIFIED:{u}")));
    for (name, value) in [("SUMMARY", &task.title), ("DESCRIPTION", &task.notes)] {
        lines.extend(
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| format!("{name}:{}", escape(v))),
        );
    }
    lines.extend(
        task.due
            .as_deref()
            .and_then(compact_date)
            .map(|d| format!("DUE;VALUE=DATE:{d}")),
    );
    let done = task.status.as_deref() == Some("completed");
    lines.push(
        match done {
            true => "STATUS:COMPLETED",
            false => "STATUS:NEEDS-ACTION",
        }
        .into(),
    );
    lines.extend(
        task.completed
            .as_deref()
            .and_then(Stamp::parse)
            .filter(|_| done)
            .map(|s| format!("COMPLETED:{}", s.utc())),
    );
    lines.extend(
        task.parent
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(|p| format!("RELATED-TO;RELTYPE=PARENT:{}", escape(p))),
    );
    lines.push("END:VTODO".into());
    lines.push("END:VCALENDAR".into());
    lines.iter().map(|l| fold(l)).collect()
}

#[cfg(test)]
mod tests;
