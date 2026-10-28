//! The HTTP over the drive: the Graph requests a drive-item replica makes, below the app folder
//! (`special/approot`) or the drive's root (`root`), answered as Graph answers them (error bodies `{"error":{"code",...}}`, `412` on a stale `If-Match`, `409
//! nameAlreadyExists` under `conflictBehavior=fail`, `507` past the quota, `410 resyncRequired`
//! for a delta token the server has dropped, upload sessions, redirected downloads).

use super::drive::{Aim, Body, CHUNK_UNIT, Drive, Node, Refused, Upload};
use crate::http::{Request, Response};
use serde_json::{Value, json};

/// What a test can change about how the server answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Knobs {
    /// A simple upload past this many bytes is refused (`413`); Graph's is 4 MB.
    pub simple_max: usize,
    /// Whether a download is answered `302` to a pre-authenticated URL.
    pub redirect_downloads: bool,
    /// How many requests are still to be answered `429`, and their `Retry-After`.
    pub throttle: Option<(u32, u32)>,
    /// How many delta items a page holds when the request names no `odata.maxpagesize`.
    pub page: usize,
    /// How many more upload-session chunks are taken before the next is answered `503` and
    /// stored nothing (the session stays, to be resumed); `None`: every one is taken.
    pub cut_upload_after: Option<u32>,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            simple_max: 4_000_000,
            redirect_downloads: false,
            throttle: None,
            page: 200,
            cut_upload_after: None,
        }
    }
}

/// The drive, the knobs, and who may ask.
#[derive(Debug, Default)]
pub struct State {
    /// The items.
    pub drive: Drive,
    /// The knobs.
    pub knobs: Knobs,
    /// Who `GET /v1.0/me` says the account is: its mail address (empty: [`DEFAULT_MAIL`]).
    pub mail: String,
    /// The calendars and their events.
    pub calendars: super::calendar::Calendars,
}

/// The address `GET /v1.0/me` names when a test set none.
pub const DEFAULT_MAIL: &str = "ada@graph.fake.test";

const DRIVE: &str = "/v1.0/me/drive";

fn error(status: u16, code: &str) -> Response {
    Response::json(status, &json!({"error": {"code": code, "message": code}}))
}

/// `GET /v1.0/me`: who the token is for, the fields the Microsoft sign-in reads (`mail`, else
/// `userPrincipalName`) and a display name.
fn me(mail: &str) -> Response {
    let mail = if mail.is_empty() { DEFAULT_MAIL } else { mail };
    Response::json(
        200,
        &json!({
            "id": "fake-user",
            "displayName": "Ada",
            "mail": mail,
            "userPrincipalName": mail,
        }),
    )
}

fn item_json(drive: &Drive, node: &Node) -> Value {
    let parent = node
        .parent
        .as_deref()
        .map(|id| json!({"id": id, "driveId": "drv"}));
    let mut item = json!({
        "id": node.id,
        "name": node.name,
        "size": node.size(),
        "eTag": node.etag(),
        "cTag": node.etag(),
    });
    if let Some(parent) = parent {
        item["parentReference"] = parent;
    }
    if node.deleted {
        item["deleted"] = json!({"state": "deleted"});
        return item;
    }
    match &node.body {
        Body::Folder => {
            let kids = drive_children(drive, &node.id);
            item["folder"] = json!({"childCount": kids});
        }
        Body::File(_) => {
            item["file"] = json!({"hashes": {"quickXorHash": node.quick_xor()}});
        }
    }
    item
}

fn drive_children(drive: &Drive, id: &str) -> usize {
    drive
        .changes(id, 0, drive.seq())
        .iter()
        .filter(|n| n.parent.as_deref() == Some(id))
        .count()
}

/// What a request addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Address {
    /// A path below the folder `.0` (the app folder for `special/approot`, the root for
    /// `root`); no names is that folder.
    Below(String, Vec<String>),
    /// `items/{id}`.
    Item(String),
}

/// An address and what is asked of it (`content`, `delta`, ...; empty for the item itself).
fn route(drive: &Drive, path: &str) -> Option<(Address, String)> {
    let rest = path.strip_prefix(DRIVE)?.strip_prefix('/')?;
    let tops = [
        ("special/approot", drive.approot()),
        ("root", super::drive::ROOT),
    ];
    for (top, from) in tops {
        let Some(rest) = rest.strip_prefix(top) else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with(':')) {
            continue;
        }
        let from = from.to_owned();
        return match rest.strip_prefix(":/") {
            Some(after) => {
                let (names, op) = after.split_once(':').unwrap_or((after, ""));
                let names = names
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(porter_ids::percent::decode)
                    .collect();
                Some((
                    Address::Below(from, names),
                    op.trim_start_matches('/').to_owned(),
                ))
            }
            None => Some((
                Address::Below(from, Vec::new()),
                rest.trim_start_matches('/').to_owned(),
            )),
        };
    }
    let rest = rest.strip_prefix("items/")?;
    if let Some((id, after)) = rest.split_once(":/") {
        // `items/{parent}:/{names}:/{op}`: a path below the folder `parent`.
        let (names, op) = after.split_once(':').unwrap_or((after, ""));
        let names = names
            .split('/')
            .filter(|s| !s.is_empty())
            .map(porter_ids::percent::decode)
            .collect();
        return Some((
            Address::Below(porter_ids::percent::decode(id), names),
            op.trim_start_matches('/').to_owned(),
        ));
    }
    let (id, op) = rest.split_once('/').unwrap_or((rest, ""));
    Some((
        Address::Item(porter_ids::percent::decode(id)),
        op.to_owned(),
    ))
}

/// Whether OneDrive takes `name`: here, the characters it refuses and a leading or trailing
/// space (`400 invalidRequest` otherwise).
fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['"', '*', ':', '<', '>', '?', '/', '\\', '|'])
        && name.trim() == name
}

fn matches_etag(node: &Node, if_match: Option<&str>) -> bool {
    if_match.is_none_or(|tag| tag == node.etag())
}

fn fails(request: &Request, body: Option<&Value>) -> bool {
    let from_query = request.query_value("@microsoft.graph.conflictBehavior");
    let from_body = body
        .and_then(|b| b.get("item"))
        .and_then(|i| i.get("@microsoft.graph.conflictBehavior"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    from_query.or(from_body).as_deref() == Some("fail")
}

fn refused(why: Refused) -> Response {
    match why {
        Refused::Full => error(507, "quotaLimitReached"),
        Refused::Blocked => error(409, "nameAlreadyExists"),
    }
}

fn written(drive: &Drive, id: &str, status: u16) -> Response {
    drive.live(id).map_or_else(
        || error(500, "generalException"),
        |node| Response::json(status, &item_json(drive, node)),
    )
}

/// A write of `bytes` to `address`, after the checks both a simple upload and a finished
/// session make.
fn store(
    drive: &mut Drive,
    address: &Address,
    fail: bool,
    if_match: Option<&str>,
    bytes: Vec<u8>,
) -> Response {
    match address {
        Address::Item(id) => {
            let Some(node) = drive.live(id) else {
                return error(404, "itemNotFound");
            };
            if !matches_etag(node, if_match) {
                return error(412, "preconditionFailed");
            }
            match drive.replace(id, bytes) {
                Ok(()) => written(drive, id, 200),
                Err(why) => refused(why),
            }
        }
        Address::Below(from, names) => {
            let Some((name, folders)) = names.split_last() else {
                return error(400, "invalidRequest");
            };
            if !name_ok(name) {
                return error(400, "invalidRequest");
            }
            if let Some(node) = drive.at_from(from, names) {
                if fail {
                    return error(409, "nameAlreadyExists");
                }
                if !matches_etag(node, if_match) {
                    return error(412, "preconditionFailed");
                }
            } else if if_match.is_some() {
                return error(404, "itemNotFound");
            }
            let existed = drive.at_from(from, names).is_some();
            let parent = match drive.ensure(from, folders) {
                Ok(parent) => parent,
                Err(why) => return refused(why),
            };
            match drive.write(&parent, name, bytes) {
                Ok(id) => written(drive, &id, if existed { 200 } else { 201 }),
                Err(why) => refused(why),
            }
        }
    }
}

fn aim_of(drive: &Drive, address: &Address) -> Aim {
    match address {
        Address::Item(id) => Aim::Item(id.clone()),
        Address::Below(from, names) => match drive.at_from(from, names) {
            Some(node) => Aim::Item(node.id.clone()),
            None => {
                let (name, folders) = names
                    .split_last()
                    .map(|(name, folders)| (name.clone(), folders.to_vec()))
                    .unwrap_or_default();
                Aim::New(from.clone(), folders, name)
            }
        },
    }
}

fn content_range(request: &Request) -> Option<(usize, usize, usize)> {
    let value = request.header("content-range")?.strip_prefix("bytes ")?;
    let (span, total) = value.split_once('/')?;
    let (first, last) = span.split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?, total.parse().ok()?))
}

fn slice(bytes: &[u8], range: Option<&str>) -> Response {
    let Some(span) = range.and_then(|r| r.strip_prefix("bytes=")) else {
        return Response::new(200).typed("application/octet-stream", bytes);
    };
    let (first, last) = span.split_once('-').unwrap_or((span, ""));
    let first: usize = first.parse().unwrap_or(0);
    if first >= bytes.len() {
        return Response::new(416);
    }
    let last = last
        .parse::<usize>()
        .map_or(bytes.len() - 1, |l| l.min(bytes.len() - 1));
    Response::new(206)
        .with_header(
            "Content-Range",
            &format!("bytes {first}-{last}/{}", bytes.len()),
        )
        .typed("application/octet-stream", &bytes[first..=last])
}

fn content_of(state: &State, link: &str, node: &Node, request: &Request) -> Response {
    let Body::File(bytes) = &node.body else {
        return error(400, "invalidRequest");
    };
    if state.knobs.redirect_downloads {
        return Response::new(302).with_header("Location", &format!("{link}/dl/{}", node.id));
    }
    slice(bytes, request.header("range"))
}

fn delta(state: &State, base: &str, scope: &str, request: &Request) -> Response {
    let drive = &state.drive;
    // Page size is the `Prefer: odata.maxpagesize=N` header, which Graph honours on every request.
    let page = request
        .header("prefer")
        .and_then(|p| {
            p.split(',')
                .find_map(|v| v.trim().strip_prefix("odata.maxpagesize="))
        })
        .and_then(|n| n.parse().ok())
        .unwrap_or(state.knobs.page)
        .max(1);
    // `token=N`: what changed after N. `skiptoken=from.end.offset`: a page of a walk begun at
    // `from` that ends at `end`. No token: everything there is now.
    let (from, end, offset) = if let Some(token) = request.query_value("token") {
        match token.parse::<u64>() {
            Ok(from) if drive.resumes(from) => (from, drive.seq(), 0),
            _ => return error(410, "resyncRequired"),
        }
    } else if let Some(skip) = request.query_value("skiptoken") {
        let parts: Vec<u64> = skip.split('.').filter_map(|p| p.parse().ok()).collect();
        match parts.as_slice() {
            [from, end, offset] if *from == 0 || drive.resumes(*from) => {
                (*from, *end, usize::try_from(*offset).unwrap_or(0))
            }
            _ => return error(410, "resyncRequired"),
        }
    } else {
        (0, drive.seq(), 0)
    };
    let all = drive.changes(scope, from, end);
    let items: Vec<Value> = all
        .iter()
        .skip(offset)
        .take(page)
        .map(|node| item_json(drive, node))
        .collect();
    let target = format!("{base}{}", request.path());
    let mut body = json!({"value": items});
    if offset + page < all.len() {
        body["@odata.nextLink"] =
            json!(format!("{target}?skiptoken={from}.{end}.{}", offset + page));
    } else {
        body["@odata.deltaLink"] = json!(format!("{target}?token={end}"));
    }
    Response::json(200, &body)
}

fn quota(drive: &Drive) -> Response {
    let used = drive.used();
    let mut quota = json!({"used": used, "deleted": 0, "state": "normal"});
    if let Some(total) = drive.limit() {
        quota["total"] = json!(total);
        quota["remaining"] = json!(total.saturating_sub(used));
    }
    Response::json(
        200,
        &json!({"id": "drv", "driveType": "personal", "quota": quota}),
    )
}

fn create_session(state: &mut State, link: &str, address: &Address, request: &Request) -> Response {
    let body: Option<Value> = serde_json::from_slice(&request.body).ok();
    let fail = fails(request, body.as_ref());
    let if_match = request.header("if-match").map(str::to_owned);
    let drive = &mut state.drive;
    let aim = aim_of(drive, address);
    match &aim {
        Aim::Item(id) => match drive.live(id) {
            None => return error(404, "itemNotFound"),
            Some(_) if matches!(address, Address::Below(..)) && fail => {
                return error(409, "nameAlreadyExists");
            }
            Some(node) if !matches_etag(node, if_match.as_deref()) => {
                return error(412, "preconditionFailed");
            }
            Some(_) => {}
        },
        Aim::New(..) if if_match.is_some() => return error(404, "itemNotFound"),
        Aim::New(..) => {}
    }
    let id = drive.open_upload(Upload {
        aim,
        fail_if_exists: fail,
        received: Vec::new(),
    });
    Response::json(
        200,
        &json!({"uploadUrl": format!("{link}/upload/{id}"), "expirationDateTime": "2099-01-01T00:00:00Z"}),
    )
}

fn upload_chunk(state: &mut State, session: &str, request: &Request) -> Response {
    let Some((first, last, total)) = content_range(request) else {
        return error(400, "invalidRange");
    };
    if state.drive.upload_mut(session).is_none() {
        return error(404, "itemNotFound");
    }
    if let Some(left) = state.knobs.cut_upload_after {
        state.knobs.cut_upload_after = left.checked_sub(1);
        if left == 0 {
            return error(503, "serviceNotAvailable");
        }
    }
    let drive = &mut state.drive;
    let Some(upload) = drive.upload_mut(session) else {
        return error(404, "itemNotFound");
    };
    let len = request.body.len();
    if first != upload.received.len() || last + 1 != first + len || last >= total {
        return error(416, "invalidRange");
    }
    let finished = last + 1 == total;
    if !finished && !len.is_multiple_of(CHUNK_UNIT) {
        return error(400, "invalidRange");
    }
    upload.received.extend_from_slice(&request.body);
    if !finished {
        let next = upload.received.len();
        return Response::json(
            202,
            &json!({"expirationDateTime": "2099-01-01T00:00:00Z", "nextExpectedRanges": [format!("{next}-")]}),
        );
    }
    let Some(done) = drive.close_upload(session) else {
        return error(404, "itemNotFound");
    };
    let address = match done.aim {
        Aim::Item(id) => Address::Item(id),
        Aim::New(from, mut folders, name) => {
            folders.push(name);
            Address::Below(from, folders)
        }
    };
    // Graph looks at `If-Match` when the session is made, not when its last bytes come (the
    // docs of createUploadSession name it there only): a file changed meanwhile is replaced.
    store(drive, &address, done.fail_if_exists, None, done.received)
}

fn children(state: &mut State, address: &Address, request: &Request) -> Response {
    let drive = &mut state.drive;
    let parent = match address {
        Address::Item(id) => id.clone(),
        Address::Below(from, names) => match drive.at_from(from, names) {
            Some(node) => node.id.clone(),
            None => return error(404, "itemNotFound"),
        },
    };
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let Some(name) = body.get("name").and_then(Value::as_str) else {
        return error(400, "invalidRequest");
    };
    if drive.live(&parent).is_none() {
        return error(404, "itemNotFound");
    }
    if !name_ok(name) {
        return error(400, "invalidRequest");
    }
    match drive.mkdir(&parent, name) {
        Some(id) => written(drive, &id, 201),
        None => error(409, "nameAlreadyExists"),
    }
}

/// `PATCH` of an item: a new name and/or a new parent (`parentReference.id`), with `If-Match`
/// checked and a name taken in the folder refused (`409`).
fn relocate(drive: &mut Drive, id: Option<String>, request: &Request) -> Response {
    let Some(id) = id else {
        return error(404, "itemNotFound");
    };
    let Some(node) = drive.live(&id) else {
        return error(404, "itemNotFound");
    };
    if !matches_etag(node, request.header("if-match")) {
        return error(412, "preconditionFailed");
    }
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let name = body["name"].as_str().unwrap_or(&node.name).to_owned();
    let parent = body["parentReference"]["id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| node.parent.clone())
        .unwrap_or_default();
    if !name_ok(&name) {
        return error(400, "invalidRequest");
    }
    if drive
        .child(&parent, &name)
        .is_some_and(|other| other.id != id)
    {
        return error(409, "nameAlreadyExists");
    }
    match drive.relocate(&id, &parent, &name) {
        true => written(drive, &id, 200),
        false => error(400, "invalidRequest"),
    }
}

/// Where the pre-authenticated links a drive hands out point.
#[derive(Debug, Clone, Copy)]
pub struct Origins<'a> {
    /// This server's origin.
    pub base: &'a str,
    /// The origin of the links (`/dl/..`, `/upload/..`): this server's own, or another one.
    pub link: &'a str,
}

/// Answers a request to the origin of the links: downloads and upload sessions, pre-authenticated,
/// so they carry no bearer. With `strict`, a request that carries any `Authorization` is refused
/// (`401`): OneDrive refuses a bearer sent to such a host.
pub fn answer_link(state: &mut State, strict: bool, request: &Request) -> Response {
    let path = request.path().to_owned();
    if strict && request.header("authorization").is_some() {
        return error(401, "InvalidAuthenticationToken");
    }
    if let Some(id) = path.strip_prefix("/dl/") {
        return match state.drive.live(id) {
            Some(node) => match &node.body {
                Body::File(bytes) => slice(bytes, request.header("range")),
                Body::Folder => error(400, "invalidRequest"),
            },
            None => error(404, "itemNotFound"),
        };
    }
    if let Some(session) = path.strip_prefix("/upload/") {
        return match request.method.as_str() {
            "PUT" => upload_chunk(state, session, request),
            "GET" => match state.drive.upload_mut(session) {
                Some(upload) => Response::json(
                    200,
                    &json!({
                        "expirationDateTime": "2099-01-01T00:00:00Z",
                        "nextExpectedRanges": [format!("{}-", upload.received.len())],
                    }),
                ),
                None => error(404, "itemNotFound"),
            },
            "DELETE" => {
                state.drive.close_upload(session);
                Response::new(204)
            }
            _ => error(405, "methodNotAllowed"),
        };
    }
    error(404, "itemNotFound")
}

/// Answers `request` to the drive's own origin. When the links point at another origin, this one
/// does not serve them.
pub fn answer(
    state: &mut State,
    origins: Origins<'_>,
    accepts: &super::Accepts,
    request: &Request,
) -> Response {
    let path = request.path().to_owned();
    let own_links = origins.base == origins.link;
    if own_links && (path.starts_with("/dl/") || path.starts_with("/upload/")) {
        return answer_link(state, false, request);
    }
    if !accepts.admits(request.bearer()) {
        return error(401, "InvalidAuthenticationToken");
    }
    if let Some((left, after)) = state.knobs.throttle {
        state.knobs.throttle = left.checked_sub(1).filter(|l| *l > 0).map(|l| (l, after));
        return error(429, "activityLimitReached").with_header("Retry-After", &after.to_string());
    }
    if path == DRIVE {
        return quota(&state.drive);
    }
    if path == "/v1.0/me" && request.method == "GET" {
        return me(&state.mail);
    }
    if let Some(response) = super::calendar::answer(
        &state.calendars,
        origins.base,
        state.knobs.page,
        &path,
        request,
    ) {
        return response;
    }
    let Some((address, op)) = route(&state.drive, &path) else {
        return error(404, "itemNotFound");
    };
    let method = request.method.as_str();
    let node = match &address {
        Address::Item(id) => state.drive.live(id),
        Address::Below(from, names) => state.drive.at_from(from, names),
    };
    let if_match = request.header("if-match");
    match (method, op.as_str()) {
        ("GET", "") => node.map_or_else(
            || error(404, "itemNotFound"),
            |n| Response::json(200, &item_json(&state.drive, n)),
        ),
        ("GET", "content") => node.map_or_else(
            || error(404, "itemNotFound"),
            |n| content_of(state, origins.link, n, request),
        ),
        ("GET", "delta") => node.map_or_else(
            || error(404, "itemNotFound"),
            |n| delta(state, origins.base, &n.id, request),
        ),
        ("DELETE", "") => match node.map(|n| (n.id.clone(), matches_etag(n, if_match))) {
            None => error(404, "itemNotFound"),
            Some((_, false)) => error(412, "preconditionFailed"),
            Some((id, true)) => {
                // OneDrive keeps what is deleted in its recycle bin.
                state.drive.recycle(&id);
                Response::new(204)
            }
        },
        ("PATCH", "") => {
            let id = node.map(|n| n.id.clone());
            relocate(&mut state.drive, id, request)
        }
        ("PUT", "content") => {
            if request.body.len() > state.knobs.simple_max {
                return error(413, "requestEntityTooLarge");
            }
            let fail = fails(request, None);
            store(
                &mut state.drive,
                &address,
                fail,
                if_match,
                request.body.clone(),
            )
        }
        ("POST", "createUploadSession") => create_session(state, origins.link, &address, request),
        ("POST", "children") => children(state, &address, request),
        _ => error(404, "itemNotFound"),
    }
}
