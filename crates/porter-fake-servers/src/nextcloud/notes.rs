//! The Notes API (`/index.php/apps/notes/api/v1/notes`): list with an ETag, get, create, update
//! and delete.

use crate::http::{Request, Response};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const ROOT: &str = "/index.php/apps/notes/api/v1/notes";

#[derive(Debug, Clone)]
struct Note {
    content: String,
    category: String,
    modified: u64,
}

/// The notes and a change counter.
#[derive(Debug, Default)]
pub struct Notes {
    notes: BTreeMap<u64, Note>,
    next_id: u64,
    clock: u64,
}

fn title_of(content: &str) -> &str {
    content
        .lines()
        .next()
        .unwrap_or("")
        .trim_start_matches('#')
        .trim()
}

impl Notes {
    fn render(&self, id: u64, note: &Note) -> Value {
        json!({
            "id": id, "etag": format!("{:x}", note.modified), "readonly": false,
            "content": note.content, "title": title_of(&note.content),
            "category": note.category, "favorite": false, "modified": note.modified,
        })
    }

    fn store(&mut self, id: u64, body: &Value) -> Value {
        self.clock += 1;
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
        let old = self.notes.get(&id);
        let note = Note {
            content: text("content")
                .or_else(|| old.map(|n| n.content.clone()))
                .unwrap_or_default(),
            category: text("category")
                .or_else(|| old.map(|n| n.category.clone()))
                .unwrap_or_default(),
            modified: self.clock,
        };
        let out = self.render(id, &note);
        self.notes.insert(id, note);
        out
    }

    pub fn route(&mut self, request: &Request) -> Response {
        let id = request
            .path()
            .strip_prefix(ROOT)
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(|rest| rest.parse::<u64>().ok());
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        match (request.method.as_str(), id) {
            ("GET", None) => {
                let etag = format!("\"{:x}\"", self.clock);
                if request.header("if-none-match") == Some(etag.as_str()) {
                    return Response::new(304).with_header("ETag", &etag);
                }
                let all: Vec<Value> = self
                    .notes
                    .iter()
                    .map(|(id, n)| self.render(*id, n))
                    .collect();
                Response::json(200, &Value::Array(all)).with_header("ETag", &etag)
            }
            ("GET", Some(id)) => match self.notes.get(&id) {
                Some(note) => Response::json(200, &self.render(id, note)),
                None => Response::new(404),
            },
            ("POST", None) => {
                self.next_id += 1;
                let id = self.next_id;
                Response::json(200, &self.store(id, &body))
            }
            ("PUT", Some(id)) if self.notes.contains_key(&id) => {
                Response::json(200, &self.store(id, &body))
            }
            ("DELETE", Some(id)) => match self.notes.remove(&id) {
                Some(_) => {
                    self.clock += 1;
                    Response::json(200, &json!({}))
                }
                None => Response::new(404),
            },
            _ => Response::new(404),
        }
    }
}
