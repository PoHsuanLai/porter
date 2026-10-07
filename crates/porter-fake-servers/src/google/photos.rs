//! Google Photos behind the fake Google: the Library API's append-only upload (bytes to an
//! upload token, then `mediaItems:batchCreate`, into an album the app created) and the Picker
//! API (a session the person fills in the browser, polled, its picked items listed and their
//! `baseUrl`s downloaded with the bearer, the session deleted when done).
//!
//! The paths are Google's own: the Library's `/v1/uploads`, `/v1/mediaItems:batchCreate` and
//! `/v1/albums`; the Picker's `/v1/sessions`, `GET /v1/mediaItems?sessionId=` and, for a picked
//! item's bytes, `/dl/<id>` (the `baseUrl`), which wants the bearer as Google's does.

use crate::http::{Request, Response};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One item in the library: what an upload made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    /// Its id.
    pub id: String,
    /// The name it was uploaded under.
    pub filename: String,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// The album it was added to.
    pub album: Option<String>,
}

/// An album the app created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    /// Its id.
    pub id: String,
    /// Its title.
    pub title: String,
}

/// What the person picks in a Picker session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// The file name.
    pub filename: String,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// The mime type (`video/...` makes it a video).
    pub mime: String,
}

/// A Picker session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Its id.
    pub id: String,
    /// Whether the person has finished picking.
    pub set: bool,
    /// Whether the app deleted it.
    pub deleted: bool,
    /// The ids of what was picked.
    pub picked: Vec<String>,
}

/// The library and the picker.
#[derive(Debug, Default)]
pub struct Photos {
    tokens: BTreeMap<String, (Vec<u8>, bool)>,
    /// What uploads made, oldest first.
    pub items: Vec<MediaItem>,
    /// The albums created.
    pub albums: Vec<Album>,
    /// The Picker sessions, oldest first.
    pub sessions: Vec<Session>,
    picks: BTreeMap<String, Pick>,
    next: u64,
}

impl Photos {
    fn fresh(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{:05}", self.next)
    }

    /// The person finishes picking in `session`: these items are its picks.
    pub fn pick(&mut self, session: &str, picks: Vec<Pick>) -> bool {
        let ids: Vec<String> = picks
            .into_iter()
            .map(|pick| {
                let id = self.fresh("pick");
                self.picks.insert(id.clone(), pick);
                id
            })
            .collect();
        match self
            .sessions
            .iter_mut()
            .find(|s| s.id == session && !s.deleted)
        {
            Some(found) => {
                found.picked = ids;
                found.set = true;
                true
            }
            None => false,
        }
    }
}

fn status(code: u16, reason: &str) -> Response {
    Response::json(
        code,
        &json!({"error": {"code": code, "message": reason, "status": reason}}),
    )
}

fn session_json(base: &str, session: &Session) -> Value {
    json!({
        "id": session.id,
        "pickerUri": format!("{base}/picker/{}", session.id),
        "pollingConfig": {"pollInterval": "1s", "timeoutIn": "600s"},
        "expireTime": "2099-01-01T00:00:00Z",
        "mediaItemsSet": session.set,
    })
}

fn batch_create(photos: &mut Photos, request: &Request) -> Response {
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let album = body["albumId"].as_str().map(str::to_owned);
    if album
        .as_deref()
        .is_some_and(|a| !photos.albums.iter().any(|known| known.id == a))
    {
        return status(400, "INVALID_ARGUMENT");
    }
    let Some(entries) = body["newMediaItems"].as_array() else {
        return status(400, "INVALID_ARGUMENT");
    };
    let mut results = Vec::new();
    for entry in entries {
        let token = entry["simpleMediaItem"]["uploadToken"]
            .as_str()
            .unwrap_or_default();
        let name = entry["simpleMediaItem"]["fileName"]
            .as_str()
            .unwrap_or("upload");
        let bytes = match photos.tokens.get_mut(token) {
            Some((bytes, used)) if !*used => {
                *used = true;
                Some(bytes.clone())
            }
            _ => None,
        };
        results.push(match bytes {
            Some(bytes) => {
                let id = photos.fresh("media");
                photos.items.push(MediaItem {
                    id: id.clone(),
                    filename: name.to_owned(),
                    bytes,
                    album: album.clone(),
                });
                json!({
                    "uploadToken": token,
                    "status": {"message": "Success"},
                    "mediaItem": {"id": id, "filename": name, "mimeType": "image/jpeg"},
                })
            }
            None => json!({
                "uploadToken": token,
                "status": {"code": 3, "message": "Failed: There was an error while trying to create this media item."},
            }),
        });
    }
    Response::json(200, &json!({"newMediaItemResults": results}))
}

fn picked_list(photos: &Photos, base: &str, request: &Request) -> Response {
    let Some(session) = request
        .query_value("sessionId")
        .and_then(|id| photos.sessions.iter().find(|s| s.id == id && !s.deleted))
    else {
        return status(404, "NOT_FOUND");
    };
    let size = request
        .query_value("pageSize")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(100);
    let from = request
        .query_value("pageToken")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(0);
    let items: Vec<Value> = session
        .picked
        .iter()
        .skip(from)
        .take(size)
        .filter_map(|id| photos.picks.get(id).map(|pick| (id, pick)))
        .map(|(id, pick)| {
            json!({
                "id": id,
                "createTime": "2026-01-01T00:00:00Z",
                "type": if pick.mime.starts_with("video/") { "VIDEO" } else { "PHOTO" },
                "mediaFile": {
                    "baseUrl": format!("{base}/dl/{id}"),
                    "mimeType": pick.mime,
                    "filename": pick.filename,
                },
            })
        })
        .collect();
    let mut body = json!({"mediaItems": items});
    if from + size < session.picked.len() {
        body["nextPageToken"] = json!((from + size).to_string());
    }
    Response::json(200, &body)
}

/// Answers a Photos request, or `None` for a path that is not one of the two APIs'.
pub fn answer(photos: &mut Photos, base: &str, request: &Request) -> Option<Response> {
    let path = request.path();
    let method = request.method.as_str();
    if let Some(rest) = path.strip_prefix("/dl/") {
        // `=d` (bytes) or `=dv` (video bytes) is the suffix Google's baseUrls take.
        let id = rest.split('=').next().unwrap_or(rest);
        return Some(match photos.picks.get(id) {
            Some(pick) => Response::new(200).typed(&pick.mime, pick.bytes.clone()),
            None => status(404, "NOT_FOUND"),
        });
    }
    let rest = path.strip_prefix("/v1/")?;
    Some(match (method, rest) {
        ("POST", "uploads") => {
            if request.header("x-goog-upload-protocol") != Some("raw") {
                return Some(status(400, "INVALID_ARGUMENT"));
            }
            let token = photos.fresh("upl");
            photos
                .tokens
                .insert(token.clone(), (request.body.clone(), false));
            Response::new(200).typed("text/plain", token)
        }
        ("POST", "mediaItems:batchCreate") => batch_create(photos, request),
        ("POST", "albums") => {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let Some(title) = body["album"]["title"].as_str() else {
                return Some(status(400, "INVALID_ARGUMENT"));
            };
            let id = photos.fresh("album");
            photos.albums.push(Album {
                id: id.clone(),
                title: title.to_owned(),
            });
            Response::json(
                200,
                &json!({"id": id, "title": title, "isWriteable": true, "productUrl": format!("{base}/album/{id}")}),
            )
        }
        ("POST", "sessions") => {
            let id = photos.fresh("session");
            let session = Session {
                id,
                set: false,
                deleted: false,
                picked: Vec::new(),
            };
            let body = session_json(base, &session);
            photos.sessions.push(session);
            Response::json(200, &body)
        }
        ("GET", "mediaItems") => picked_list(photos, base, request),
        (_, sessions) => {
            let id = sessions.strip_prefix("sessions/")?;
            let found = photos
                .sessions
                .iter_mut()
                .find(|s| s.id == id && !s.deleted);
            match (method, found) {
                (_, None) => status(404, "NOT_FOUND"),
                ("GET", Some(session)) => Response::json(200, &session_json(base, session)),
                ("DELETE", Some(session)) => {
                    session.deleted = true;
                    Response::json(200, &json!({}))
                }
                _ => status(405, "METHOD_NOT_ALLOWED"),
            }
        }
    })
}
