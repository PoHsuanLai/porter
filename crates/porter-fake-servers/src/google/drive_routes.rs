//! The HTTP over the drive: the Drive v3 requests a replica of the app data folder makes,
//! answered as Drive answers them (`{"error":{"code","errors":[{"reason"}]}}` bodies,
//! `storageQuotaExceeded` past the limit, `308` between resumable chunks, a page token the
//! server dropped refused `400`).

use super::drive::{ALIAS, Aim, CHUNK_UNIT, Drive, FOLDER, Node, Refused, checksum};
use crate::http::{Request, Response, percent_decode};
use serde_json::{Value, json};

/// How the fake Drive answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveKnobs {
    /// How many changes a `changes.list` page holds when the request names no `pageSize`.
    pub page: usize,
}

impl Default for DriveKnobs {
    fn default() -> Self {
        Self { page: 100 }
    }
}

fn error(status: u16, reason: &str) -> Response {
    Response::json(
        status,
        &json!({"error": {"code": status, "message": reason, "errors": [{"reason": reason}]}}),
    )
}

/// The file resource of a node.
pub fn file_json(drive: &Drive, node: &Node) -> Value {
    let mut file = json!({
        "kind": "drive#file",
        "id": node.id,
        "name": node.name,
        "mimeType": if node.is_folder() { FOLDER } else { "application/octet-stream" },
        "version": node.version.to_string(),
        "trashed": node.trashed,
        "spaces": [ALIAS],
    });
    if let Some(parent) = &node.parent {
        file["parents"] = json!([parent]);
    }
    if let Some(bytes) = &node.bytes {
        file["size"] = json!(bytes.len().to_string());
        if let Some(sum) = checksum(node) {
            file["md5Checksum"] = json!(sum);
        }
    }
    let _ = drive;
    file
}

fn refusal(why: Refused) -> Response {
    match why {
        Refused::Full => error(403, "storageQuotaExceeded"),
        Refused::NoParent => error(404, "notFound"),
    }
}

/// The parent a create names: its first `parents` entry, which must be given (an app with only
/// the app data scope cannot write anywhere else).
fn parent_of(meta: &Value) -> Option<String> {
    meta["parents"]
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_owned)
}

/// One `q` term that a node must satisfy.
fn term_admits(term: &str, node: &Node) -> bool {
    let term = term.trim();
    let quoted = |s: &str| -> Option<String> {
        let s = s.trim().strip_prefix('\'')?.strip_suffix('\'')?;
        Some(s.replace("\\'", "'").replace("\\\\", "\\"))
    };
    if let Some(rest) = term.strip_suffix(" in parents") {
        return quoted(rest).is_some_and(|p| node.parent.as_deref() == Some(p.as_str()));
    }
    if let Some((field, value)) = term.split_once(" != ") {
        return match field.trim() {
            "mimeType" => quoted(value).is_none_or(|m| (m == FOLDER) != node.is_folder()),
            _ => true,
        };
    }
    if let Some((field, value)) = term.split_once(" = ") {
        return match field.trim() {
            "name" => quoted(value).is_some_and(|n| n == node.name),
            "trashed" => (value.trim() == "true") == node.trashed,
            "mimeType" => quoted(value).is_some_and(|m| (m == FOLDER) == node.is_folder()),
            _ => true,
        };
    }
    true
}

/// Splits a `q` on its top-level ` and `, a quoted value being left whole.
fn terms(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let chars: Vec<char> = q.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && quoted && i + 1 < chars.len() {
            current.push(c);
            current.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '\'' {
            quoted = !quoted;
        }
        let rest: String = chars[i..].iter().take(5).collect();
        if !quoted && rest == " and " {
            out.push(std::mem::take(&mut current));
            i += 5;
            continue;
        }
        current.push(c);
        i += 1;
    }
    out.push(current);
    out
}

fn list(drive: &Drive, request: &Request) -> Response {
    let q = request.query_value("q").unwrap_or_default();
    let wants = terms(&q);
    let mut nodes: Vec<&Node> = drive
        .nodes_listed()
        .into_iter()
        .filter(|n| {
            wants
                .iter()
                .filter(|t| !t.trim().is_empty())
                .all(|t| term_admits(t, n))
        })
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let size = request
        .query_value("pageSize")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(100);
    let from = request
        .query_value("pageToken")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(0);
    let page: Vec<Value> = nodes
        .iter()
        .skip(from)
        .take(size)
        .map(|n| file_json(drive, n))
        .collect();
    let mut body = json!({"kind": "drive#fileList", "files": page});
    if from + size < nodes.len() {
        body["nextPageToken"] = json!((from + size).to_string());
    }
    Response::json(200, &body)
}

fn change(drive: &Drive, node: &Node) -> Value {
    match node.removed {
        true => json!({
            "kind": "drive#change", "changeType": "file", "type": "file",
            "fileId": node.id, "removed": true,
        }),
        false => json!({
            "kind": "drive#change", "changeType": "file", "type": "file",
            "fileId": node.id, "removed": false, "file": file_json(drive, node),
        }),
    }
}

fn changes(drive: &Drive, knobs: DriveKnobs, request: &Request) -> Response {
    let Some(token) = request
        .query_value("pageToken")
        .and_then(|t| t.parse::<u64>().ok())
        .filter(|t| drive.token_valid(*t))
    else {
        return error(400, "invalid");
    };
    let size = request
        .query_value("pageSize")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(knobs.page);
    let all = drive.changes_after(token);
    let page: Vec<&Node> = all.iter().copied().take(size).collect();
    let mut body = json!({
        "kind": "drive#changeList",
        "changes": page.iter().map(|n| change(drive, n)).collect::<Vec<_>>(),
    });
    match (all.len() > size, page.last()) {
        (true, Some(last)) => body["nextPageToken"] = json!(last.changed.to_string()),
        _ => body["newStartPageToken"] = json!(drive.seq().to_string()),
    }
    Response::json(200, &body)
}

fn about(drive: &Drive) -> Response {
    let mut quota =
        json!({"usage": drive.used().to_string(), "usageInDrive": drive.used().to_string()});
    if let Some(limit) = drive.limit() {
        quota["limit"] = json!(limit.to_string());
    }
    Response::json(200, &json!({"kind": "drive#about", "storageQuota": quota}))
}

/// `bytes=a-b` as an inclusive span clipped to `len`; `None` for a span past the end.
fn span(header: &str, len: usize) -> Option<(usize, usize)> {
    let range = header.strip_prefix("bytes=")?;
    let (from, to) = range.split_once('-')?;
    let from: usize = from.parse().ok()?;
    let to = to
        .parse::<usize>()
        .ok()
        .map_or(len.saturating_sub(1), |t| t.min(len.saturating_sub(1)));
    (from < len && from <= to).then_some((from, to))
}

fn media(node: &Node, request: &Request) -> Response {
    let bytes = node.bytes.as_deref().unwrap_or_default();
    match request.header("range") {
        None => Response::new(200).typed("application/octet-stream", bytes),
        Some(header) => match span(header, bytes.len()) {
            Some((from, to)) => Response::new(206)
                .with_header(
                    "Content-Range",
                    &format!("bytes {from}-{to}/{}", bytes.len()),
                )
                .typed("application/octet-stream", &bytes[from..=to]),
            None => error(416, "requestedRangeNotSatisfiable"),
        },
    }
}

/// The parts of a `multipart/related` body: each part's bytes after its headers.
fn multipart(request: &Request) -> Option<Vec<Vec<u8>>> {
    let kind = request.header("content-type")?;
    let boundary = kind.split("boundary=").nth(1)?.trim().trim_matches('"');
    let marker = format!("--{boundary}").into_bytes();
    let body = &request.body;
    let mut at: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + marker.len() <= body.len() {
        if body[i..].starts_with(&marker) {
            at.push(i);
            i += marker.len();
        } else {
            i += 1;
        }
    }
    let mut parts = Vec::new();
    for pair in at.windows(2) {
        let raw = &body[pair[0] + marker.len()..pair[1]];
        let raw = raw.strip_prefix(b"\r\n").unwrap_or(raw);
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
        let content = &raw[split + 4..];
        parts.push(content.strip_suffix(b"\r\n").unwrap_or(content).to_vec());
    }
    Some(parts)
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

/// A create or an update that carries content: `media` (content only), `multipart` (metadata and
/// content) or `resumable` (metadata now, content later).
fn upload(drive: &mut Drive, base: &str, request: &Request, over: Option<&str>) -> Response {
    let kind = request.query_value("uploadType").unwrap_or_default();
    let (meta, content) = match kind.as_str() {
        "media" => (Value::Null, request.body.clone()),
        "multipart" => match multipart(request).as_deref() {
            Some([meta, content]) => (json_of(meta), content.clone()),
            _ => return error(400, "invalid"),
        },
        "resumable" => {
            let meta = json_of(&request.body);
            let aim = match over {
                Some(id) => match drive.get(id) {
                    Some(node) => Aim::Item(node.id.clone()),
                    None => return error(404, "notFound"),
                },
                None => match (parent_of(&meta), meta["name"].as_str()) {
                    (Some(parent), Some(name)) if drive.get(&parent).is_some() => Aim::New {
                        parent,
                        name: name.to_owned(),
                    },
                    (None, _) => return error(403, "insufficientFilePermissions"),
                    _ => return error(404, "notFound"),
                },
            };
            let id = drive.open_upload(aim);
            return Response::new(200).with_header(
                "Location",
                &format!("{base}/upload/drive/v3/files?uploadType=resumable&upload_id={id}"),
            );
        }
        _ => return error(400, "badRequest"),
    };
    finish(drive, &meta, content, over)
}

fn finish(drive: &mut Drive, meta: &Value, content: Vec<u8>, over: Option<&str>) -> Response {
    let written = match over {
        Some(id) => match drive.get(id) {
            Some(node) => drive.write("", &node.name.clone(), content, Some(id)),
            None => return error(404, "notFound"),
        },
        None => match (parent_of(meta), meta["name"].as_str()) {
            (Some(parent), Some(name)) => drive.write(&parent, name, content, None),
            (None, _) => return error(403, "insufficientFilePermissions"),
            _ => return error(400, "required"),
        },
    };
    match written {
        Ok(id) => match drive.get(&id) {
            Some(node) => Response::json(200, &file_json(drive, node)),
            None => error(404, "notFound"),
        },
        Err(why) => refusal(why),
    }
}

/// `PUT` of a resumable chunk.
fn chunk(drive: &mut Drive, request: &Request) -> Response {
    let Some(id) = request.query_value("upload_id") else {
        return error(404, "notFound");
    };
    let Some(range) = request.header("content-range") else {
        return error(400, "badRequest");
    };
    let Some(spec) = range.strip_prefix("bytes ") else {
        return error(400, "badRequest");
    };
    let (part, total) = spec.split_once('/').unwrap_or((spec, "*"));
    let total: Option<usize> = total.parse().ok();
    let Some(upload) = drive.upload(&id) else {
        return error(404, "notFound");
    };
    if part != "*" {
        let start: usize = part
            .split_once('-')
            .and_then(|(s, _)| s.parse().ok())
            .unwrap_or(usize::MAX);
        if start != upload.received.len() {
            return incomplete(upload.received.len());
        }
        let last = total.is_some_and(|t| start + request.body.len() >= t);
        if !last && !request.body.len().is_multiple_of(CHUNK_UNIT) {
            return error(400, "badRequest");
        }
        upload.received.extend_from_slice(&request.body);
    }
    let have = upload.received.len();
    if total != Some(have) {
        return incomplete(have);
    }
    let Some(done) = drive.close_upload(&id) else {
        return error(404, "notFound");
    };
    match done.aim {
        Aim::Item(item) => finish(drive, &Value::Null, done.received, Some(&item)),
        Aim::New { parent, name } => finish(
            drive,
            &json!({"parents": [parent], "name": name}),
            done.received,
            None,
        ),
    }
}

fn incomplete(have: usize) -> Response {
    let response = Response::new(308);
    match have {
        0 => response,
        n => response.with_header("Range", &format!("bytes=0-{}", n - 1)),
    }
}

/// `PATCH /drive/v3/files/{id}`: a rename, a move, the trash.
fn patch(drive: &mut Drive, id: &str, request: &Request) -> Response {
    if drive.get(id).is_none() {
        return error(404, "notFound");
    }
    let meta = json_of(&request.body);
    let add = request.query_value("addParents");
    if meta["name"].is_string() || add.is_some() {
        drive.relocate(id, meta["name"].as_str(), add.as_deref());
    }
    if let Some(trashed) = meta["trashed"].as_bool() {
        drive.trash(id, trashed);
    }
    match drive.get(id) {
        Some(node) => Response::json(200, &file_json(drive, node)),
        None => error(404, "notFound"),
    }
}

/// Answers a Drive request, or `None` for a path that is not the Drive API's.
pub fn answer(
    drive: &mut Drive,
    knobs: DriveKnobs,
    base: &str,
    request: &Request,
) -> Option<Response> {
    let path = request.path();
    // Google's APIs take `X-HTTP-Method-Override` on a POST, which is how a client that has no
    // PATCH sends one.
    let method = match (
        request.method.as_str(),
        request.header("x-http-method-override"),
    ) {
        ("POST", Some(over)) => over,
        (method, _) => method,
    };
    if let Some(rest) = path.strip_prefix("/upload/drive/v3/files") {
        let id = rest.strip_prefix('/').map(percent_decode);
        return Some(match (method, id) {
            ("POST", None) => upload(drive, base, request, None),
            ("PUT", None) => chunk(drive, request),
            ("PATCH", Some(id)) => upload(drive, base, request, Some(&id)),
            _ => error(405, "methodNotAllowed"),
        });
    }
    let rest = path.strip_prefix("/drive/v3/")?;
    Some(match (method, rest) {
        ("GET", "about") => about(drive),
        ("GET", "changes/startPageToken") => {
            Response::json(200, &json!({"startPageToken": drive.seq().to_string()}))
        }
        ("GET", "changes") => changes(drive, knobs, request),
        ("GET", "files") => list(drive, request),
        ("POST", "files") => {
            let meta = json_of(&request.body);
            match (
                parent_of(&meta),
                meta["name"].as_str(),
                meta["mimeType"].as_str(),
            ) {
                (Some(parent), Some(name), Some(FOLDER)) => {
                    match drive.make_folder(&parent, name) {
                        Ok(id) => drive.get(&id).map_or_else(
                            || error(404, "notFound"),
                            |n| Response::json(200, &file_json(drive, n)),
                        ),
                        Err(why) => refusal(why),
                    }
                }
                (None, ..) => error(403, "insufficientFilePermissions"),
                _ => finish(drive, &meta, Vec::new(), None),
            }
        }
        (_, files) => {
            let id = percent_decode(files.strip_prefix("files/")?);
            match method {
                "GET" => match drive.get(&id) {
                    None => error(404, "notFound"),
                    Some(node) if request.query_value("alt").as_deref() == Some("media") => {
                        media(node, request)
                    }
                    Some(node) => Response::json(200, &file_json(drive, node)),
                },
                "PATCH" => patch(drive, &id, request),
                "DELETE" => match drive.get(&id) {
                    Some(_) => {
                        drive.delete(&id);
                        Response::new(204)
                    }
                    None => error(404, "notFound"),
                },
                _ => error(405, "methodNotAllowed"),
            }
        }
    })
}
