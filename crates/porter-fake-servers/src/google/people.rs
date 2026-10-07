//! The contacts half of the fake Google PIM APIs (People): `people/me/connections` with
//! `requestSyncToken`, a `syncToken` that returns what changed (deleted contacts carry
//! `metadata.deleted`), `EXPIRED_SYNC_TOKEN` for a token the server dropped, and `people/{id}`.

use super::pim::{error, numbers};
use crate::http::{Request, Response, percent_decode};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Row {
    resource: String,
    person: Value,
    seq: u64,
    removed: bool,
}

/// The contacts.
#[derive(Debug, Default)]
pub struct People {
    rows: Vec<Row>,
    seq: u64,
    expired_before: u64,
}

impl People {
    /// Creates or replaces the contact `resource` (`people/c1001`); the fake adds `resourceName`
    /// and `etag`.
    pub fn put(&mut self, resource: &str, mut person: Value) {
        self.seq += 1;
        if let Value::Object(map) = &mut person {
            map.insert("resourceName".into(), json!(resource));
            map.insert("etag".into(), json!(format!("%Eg{}", self.seq)));
            map.insert(
                "metadata".into(),
                json!({"sources": [{"type": "CONTACT", "id": resource.trim_start_matches("people/")}]}),
            );
        }
        let row = Row {
            resource: resource.to_owned(),
            person,
            seq: self.seq,
            removed: false,
        };
        match self.rows.iter_mut().find(|r| r.resource == resource) {
            Some(held) => *held = row,
            None => self.rows.push(row),
        }
    }

    /// Deletes a contact; the next incremental listing reports it `metadata.deleted`.
    pub fn remove(&mut self, resource: &str) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(row) = self.rows.iter_mut().find(|r| r.resource == resource) {
            row.removed = true;
            row.seq = seq;
            row.person = json!({
                "resourceName": resource,
                "metadata": {"deleted": true, "sources": [{"type": "CONTACT"}]},
            });
        }
    }

    /// Drops every sync token handed out so far.
    pub fn expire(&mut self) {
        self.expired_before = self.seq + 1;
        self.seq += 1;
    }

    /// The live contacts: resource name and JSON.
    pub fn all(&self) -> Vec<(String, Value)> {
        self.rows
            .iter()
            .filter(|r| !r.removed)
            .map(|r| (r.resource.clone(), r.person.clone()))
            .collect()
    }

    /// Two contacts a test can start from.
    pub fn seed(&mut self) {
        self.put(
            "people/c1001",
            json!({
                "names": [{"displayName": "Ada Lovelace", "familyName": "Lovelace", "givenName": "Ada"}],
                "emailAddresses": [{"value": "ada@example.test", "type": "home"}],
                "phoneNumbers": [{"value": "+44 20 7946 0000", "type": "mobile"}],
                "organizations": [{"name": "Analytical Engines", "title": "Programmer"}],
                "birthdays": [{"date": {"year": 1815, "month": 12, "day": 10}}],
            }),
        );
        self.put(
            "people/c1002",
            json!({
                "names": [{"displayName": "Grace Hopper", "familyName": "Hopper", "givenName": "Grace"}],
                "emailAddresses": [{"value": "grace@example.test", "type": "work"}],
            }),
        );
    }

    fn connections(&self, request: &Request) -> Response {
        let page = request
            .query_value("pageSize")
            .and_then(|n| n.parse().ok())
            .unwrap_or(100usize)
            .max(1);
        let wants_token = request.query_value("requestSyncToken").as_deref() == Some("true");
        let valid = |from: u64| from >= self.expired_before;
        let expired = || {
            Response::json(
                400,
                &json!({"error": {
                    "code": 400, "status": "FAILED_PRECONDITION",
                    "message": "Sync token is expired. Clear local cache and retry call without the sync token.",
                    "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
                                 "reason": "EXPIRED_SYNC_TOKEN", "domain": "googleapis.com"}],
                }}),
            )
        };
        let invalid = || error(400, "INVALID_ARGUMENT");
        let (from, end, offset) = if let Some(token) = request.query_value("pageToken") {
            match numbers(&token, "pp").as_deref() {
                Some([from, end, offset]) if *from == 0 || valid(*from) => {
                    (*from, *end, usize::try_from(*offset).unwrap_or(0))
                }
                Some(_) => return expired(),
                None => return invalid(),
            }
        } else if let Some(token) = request.query_value("syncToken") {
            match numbers(&token, "psync-").as_deref() {
                Some([from]) if valid(*from) => (*from, self.seq, 0),
                Some(_) => return expired(),
                None => return invalid(),
            }
        } else {
            (0, self.seq, 0)
        };
        let all: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| r.seq > from && r.seq <= end)
            .filter(|r| !r.removed || from > 0)
            .collect();
        let connections: Vec<Value> = all
            .iter()
            .skip(offset)
            .take(page)
            .map(|r| r.person.clone())
            .collect();
        let mut body =
            json!({"connections": connections, "totalPeople": all.len(), "totalItems": all.len()});
        if offset + page < all.len() {
            body["nextPageToken"] = json!(format!("pp{from}.{end}.{}", offset + page));
        } else if wants_token {
            body["nextSyncToken"] = json!(format!("psync-{end}"));
        }
        Response::json(200, &body)
    }

    /// Answers `rest` (what follows `/v1`), or `None` when it is not a people route.
    pub fn answer(&self, rest: &str, request: &Request) -> Option<Response> {
        let parts: Vec<&str> = rest.trim_matches('/').split('/').collect();
        let response = match parts.as_slice() {
            ["people", "me", "connections"] => self.connections(request),
            ["people", id] => {
                let resource = format!("people/{}", percent_decode(id));
                self.rows
                    .iter()
                    .find(|r| r.resource == resource)
                    .map_or_else(
                        || error(404, "NOT_FOUND"),
                        |r| Response::json(200, &r.person),
                    )
            }
            _ => return None,
        };
        Some(match request.method == "GET" {
            true => response,
            false => error(405, "methodNotAllowed"),
        })
    }
}
