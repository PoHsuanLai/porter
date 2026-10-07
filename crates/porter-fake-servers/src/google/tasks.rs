//! The Tasks half of the fake Google PIM APIs: `users/@me/lists`, a list's `tasks.list` with
//! `updatedMin`, `showDeleted`, `showHidden`, `showCompleted` and paging, and `tasks.get`. There
//! is no sync token in Tasks: a deleted task stays listable with `deleted: true`, and `updated`
//! moves with every change. `updatedMin` is inclusive here.

use super::pim::{error, numbers, stamp};
use crate::http::{Request, Response, percent_decode};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct List {
    id: String,
    title: String,
}

#[derive(Debug, Clone)]
struct Row {
    list: String,
    id: String,
    task: Value,
    seq: u64,
}

/// The task lists and their tasks.
#[derive(Debug, Default)]
pub struct Tasks {
    lists: Vec<List>,
    rows: Vec<Row>,
    seq: u64,
}

impl Tasks {
    /// Whether any list is set (until one is, the probe's one list answers).
    pub fn is_empty(&self) -> bool {
        self.lists.is_empty()
    }

    /// Adds the list `id`, or renames it.
    pub fn set_list(&mut self, id: &str, title: &str) {
        let next = List {
            id: id.to_owned(),
            title: title.to_owned(),
        };
        match self.lists.iter_mut().find(|l| l.id == id) {
            Some(held) => *held = next,
            None => self.lists.push(next),
        }
    }

    /// Removes a list and its tasks.
    pub fn remove_list(&mut self, id: &str) {
        self.lists.retain(|l| l.id != id);
        self.rows.retain(|r| r.list != id);
    }

    /// Creates or replaces a task; the fake adds `id`, `etag` and `updated`.
    pub fn put(&mut self, list: &str, id: &str, mut task: Value) {
        self.seq += 1;
        if let Value::Object(map) = &mut task {
            map.insert("kind".into(), json!("tasks#task"));
            map.insert("id".into(), json!(id));
            map.insert("etag".into(), json!(format!("\"t{}\"", self.seq)));
            map.insert("updated".into(), json!(stamp(self.seq)));
            map.entry("status").or_insert_with(|| json!("needsAction"));
        }
        let row = Row {
            list: list.to_owned(),
            id: id.to_owned(),
            task,
            seq: self.seq,
        };
        match self.rows.iter_mut().find(|r| r.list == list && r.id == id) {
            Some(held) => *held = row,
            None => self.rows.push(row),
        }
    }

    /// Deletes a task: it stays, `deleted: true`, with a new `updated`.
    pub fn remove(&mut self, list: &str, id: &str) {
        self.seq += 1;
        let (seq, updated) = (self.seq, stamp(self.seq));
        if let Some(row) = self.rows.iter_mut().find(|r| r.list == list && r.id == id) {
            row.seq = seq;
            if let Value::Object(map) = &mut row.task {
                map.insert("deleted".into(), json!(true));
                map.insert("updated".into(), json!(updated));
                map.insert("etag".into(), json!(format!("\"t{seq}\"")));
            }
        }
    }

    /// The live tasks of a list: id and JSON.
    pub fn tasks(&self, list: &str) -> Vec<(String, Value)> {
        self.rows
            .iter()
            .filter(|r| r.list == list && r.task["deleted"] != json!(true))
            .map(|r| (r.id.clone(), r.task.clone()))
            .collect()
    }

    /// Two lists a test can start from: `list-home` with a task and its subtask, `list-work`
    /// with a completed one (named Errands).
    pub fn seed(&mut self) {
        self.set_list("list-home", "Home");
        self.set_list("list-work", "Errands");
        self.put(
            "list-home",
            "t-milk",
            json!({"title": "Buy milk", "notes": "Semi-skimmed", "due": "2026-10-12T00:00:00.000Z"}),
        );
        self.put(
            "list-home",
            "t-eggs",
            json!({"title": "Eggs", "parent": "t-milk"}),
        );
        self.put(
            "list-work",
            "t-report",
            json!({"title": "File the report", "status": "completed",
                   "completed": "2026-10-07T09:15:00.000Z"}),
        );
    }

    fn lists(&self, request: &Request) -> Response {
        let page = request
            .query_value("maxResults")
            .and_then(|n| n.parse().ok())
            .unwrap_or(20usize)
            .max(1);
        let offset = request
            .query_value("pageToken")
            .and_then(|t| numbers(&t, "tl").and_then(|n| n.first().copied()))
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        let items: Vec<Value> = self
            .lists
            .iter()
            .skip(offset)
            .take(page)
            .map(|l| {
                json!({"kind": "tasks#taskList", "id": l.id, "title": l.title,
                            "updated": "2026-10-01T00:00:00.000Z"})
            })
            .collect();
        let mut body = json!({"kind": "tasks#taskLists", "items": items});
        if offset + page < self.lists.len() {
            body["nextPageToken"] = json!(format!("tl{}", offset + page));
        }
        Response::json(200, &body)
    }

    fn tasks_list(&self, list: &str, request: &Request) -> Response {
        if !self.lists.iter().any(|l| l.id == list) {
            return error(404, "notFound");
        }
        let page = request
            .query_value("maxResults")
            .and_then(|n| n.parse().ok())
            .unwrap_or(20usize)
            .max(1);
        let flag = |name: &str, default: bool| match request.query_value(name).as_deref() {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        };
        let (deleted, hidden, completed) = (
            flag("showDeleted", false),
            flag("showHidden", false),
            flag("showCompleted", true),
        );
        let min = request.query_value("updatedMin");
        let offset = request
            .query_value("pageToken")
            .and_then(|t| numbers(&t, "tp").and_then(|n| n.first().copied()))
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        let all: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| r.list == list)
            .filter(|r| deleted || r.task["deleted"] != json!(true))
            .filter(|r| hidden || r.task["hidden"] != json!(true))
            .filter(|r| completed || r.task["status"] != json!("completed"))
            .filter(|r| {
                min.as_deref()
                    .is_none_or(|min| r.task["updated"].as_str().unwrap_or("") >= min)
            })
            .collect();
        let items: Vec<Value> = all
            .iter()
            .skip(offset)
            .take(page)
            .map(|r| r.task.clone())
            .collect();
        let mut body = json!({"kind": "tasks#tasks", "items": items});
        if offset + page < all.len() {
            body["nextPageToken"] = json!(format!("tp{}", offset + page));
        }
        Response::json(200, &body)
    }

    /// Answers `rest` (what follows `/tasks/v1`), or `None` when it is not a tasks route.
    pub fn answer(&self, rest: &str, request: &Request) -> Option<Response> {
        let parts: Vec<&str> = rest.trim_matches('/').split('/').collect();
        let response = match parts.as_slice() {
            ["users", "@me", "lists"] => self.lists(request),
            ["lists", list, "tasks"] => self.tasks_list(&percent_decode(list), request),
            ["lists", list, "tasks", id] => {
                let (list, id) = (percent_decode(list), percent_decode(id));
                self.rows
                    .iter()
                    .find(|r| r.list == list && r.id == id)
                    .map_or_else(|| error(404, "notFound"), |r| Response::json(200, &r.task))
            }
            _ => return None,
        };
        Some(match request.method == "GET" {
            true => response,
            false => error(405, "methodNotAllowed"),
        })
    }
}
