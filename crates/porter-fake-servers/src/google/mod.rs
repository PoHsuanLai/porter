//! A fake Google: an OAuth issuer that behaves as Google's does (a refresh token issued once to a
//! code that asked for `access_type=offline`, never rotated, the granted scopes in every answer,
//! the application secret required, revoke) and the account APIs a Google sign-in reads: userinfo,
//! and the one list call each of Calendar, People, Tasks and Drive's app folder that the family's
//! capability probe makes. The APIs are plain HTTP on loopback behind `Authorization: Bearer`,
//! and accept the access tokens the issuer minted and has not revoked; a token the issuer issued
//! for scopes that do not include a service's gets `403`, as Google answers.
//!
//! The paths are Google's own (`/calendar/v3/users/me/calendarList`, `/v1/contactGroups`,
//! `/tasks/v1/users/@me/lists`, `/drive/v3/files`, `/v1/userinfo`), so a provider file rewritten
//! by [`FakeServer::rewrite`] keeps each row's path and only moves its origin. The calendar
//! mirror is not here: it follows the lane that needs it.
//!
//! The Drive app data folder (`drive`, `drive_routes`: files, folders, `changes.list`,
//! multipart and resumable uploads, the quota) and Google Photos (`photos`: the Library API's
//! append-only upload and the Picker API's sessions) are served beside them, with control levers
//! on [`GoogleHandle`] (another device writes or deletes a file, a person picks photos). A fake
//! made with [`FakeGoogle::bind_fixed`] accepts fixed bearers (what accountd's relay adds in a
//! bus test) instead of an issuer's tokens.

mod drive;
mod drive_routes;
mod levers;
mod md5;
mod photos;

pub use drive::{ALIAS, CHUNK_UNIT, FOLDER};
pub use drive_routes::DriveKnobs;
pub use md5::md5_hex;
pub use photos::{Album, MediaItem, Pick, Session};

use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::oauth::{FakeIssuer, IssuerHandle, Style};
use crate::seen::{Running, Seen, lock};
use porter_core::Family;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::{Endpoint, ProviderSpec};
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

/// The address `GET /v1/userinfo` names unless a test sets another.
pub const DEFAULT_ADDRESS: &str = "ada@gmail.com";

const APPDATA: &str = "https://www.googleapis.com/auth/drive.appdata";
const PHOTOS_UPLOAD: &str = "https://www.googleapis.com/auth/photoslibrary.appendonly";
const PHOTOS_PICKER: &str = "https://www.googleapis.com/auth/photospicker.mediaitems.readonly";

/// The scopes (any one of) a request wants, or none for a path this fake does not serve.
fn wants(request: &Request) -> Option<&'static [&'static str]> {
    let path = request.path();
    if path.starts_with("/drive/v3/") || path.starts_with("/upload/drive/v3/") {
        return Some(&[APPDATA]);
    }
    match path {
        "/v1/uploads" | "/v1/mediaItems:batchCreate" | "/v1/albums" => {
            return Some(&[PHOTOS_UPLOAD]);
        }
        "/v1/sessions" | "/v1/mediaItems" => return Some(&[PHOTOS_PICKER]),
        _ => {}
    }
    if path.starts_with("/v1/sessions/") || path.starts_with("/dl/") {
        return Some(&[PHOTOS_PICKER]);
    }
    match path {
        "/v1/userinfo" => Some(&[
            "https://www.googleapis.com/auth/userinfo.email",
            "email",
            "openid",
        ]),
        "/calendar/v3/users/me/calendarList" => Some(&["https://www.googleapis.com/auth/calendar"]),
        "/v1/contactGroups" => Some(&["https://www.googleapis.com/auth/contacts"]),
        "/tasks/v1/users/@me/lists" => Some(&["https://www.googleapis.com/auth/tasks"]),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct State {
    address: String,
    name: String,
    /// A status to answer a path with in place of the usual one.
    statuses: HashMap<String, u16>,
    /// Paths whose API is not switched on in the project: `403 accessNotConfigured`.
    disabled: Vec<String>,
    /// Bearers accepted as they are (no issuer, no scope check).
    fixed: Vec<String>,
    /// The Drive app data folder.
    drive: drive::Drive,
    /// How the Drive answers.
    drive_knobs: DriveKnobs,
    /// Google Photos: the library and the picker.
    photos: photos::Photos,
    /// How many requests are still to be answered `429`, and their `Retry-After`.
    throttle: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    issuer: Option<IssuerHandle>,
    state: Arc<Mutex<State>>,
    hits: Seen<Hit>,
}

/// The test's side of the running account APIs.
#[derive(Debug, Clone)]
pub struct GoogleHandle {
    shared: Shared,
}

/// The account APIs, bound and ready to serve.
#[derive(Debug)]
pub struct FakeGoogle {
    listener: Listener,
    shared: Shared,
}

impl FakeGoogle {
    /// Binds APIs that accept every access token `issuer` has minted and not revoked.
    pub async fn bind(issuer: &IssuerHandle) -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "google").await?;
        let port = crate::net::port_of(listener.address());
        let state = State {
            address: DEFAULT_ADDRESS.to_owned(),
            name: "Ada Lovelace".to_owned(),
            ..State::default()
        };
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                issuer: Some(issuer.clone()),
                state: Arc::new(Mutex::new(state)),
                hits: Seen::default(),
            },
        })
    }

    /// Binds APIs that accept `Authorization: Bearer <token>` as it is (and any more a test adds
    /// with [`GoogleHandle::accept_bearer`]): what accountd's relay adds in a bus test. There is
    /// no issuer, so no scope is checked.
    pub async fn bind_fixed(token: &str) -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "google").await?;
        let port = crate::net::port_of(listener.address());
        let state = State {
            address: DEFAULT_ADDRESS.to_owned(),
            name: "Ada Lovelace".to_owned(),
            fixed: vec![token.to_owned()],
            ..State::default()
        };
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                issuer: None,
                state: Arc::new(Mutex::new(state)),
                hits: Seen::default(),
            },
        })
    }

    /// [`FakeGoogle::bind_fixed`], serving on a task.
    pub async fn start_fixed(token: &str) -> io::Result<Running<GoogleHandle>> {
        let fake = Self::bind_fixed(token).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }

    /// The handle onto these APIs.
    pub fn handle(&self) -> GoogleHandle {
        GoogleHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start(issuer: &IssuerHandle) -> io::Result<Running<GoogleHandle>> {
        let fake = Self::bind(issuer).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl GoogleHandle {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// Where userinfo is read: `GoogleEnv::with_userinfo` takes it.
    pub fn userinfo_url(&self) -> String {
        format!("{}/v1/userinfo", self.shared.base)
    }

    /// `spec` with the Google API rows pointed here, each keeping its path.
    pub fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        point_at(&self.shared.base, spec)
    }

    /// Sets the address and name userinfo answers with.
    pub fn set_user(&self, address: &str, name: &str) {
        let mut state = lock(&self.shared.state);
        state.address = address.to_owned();
        state.name = name.to_owned();
    }

    /// Answers `path` with `status` from now on (`403` is a service the account is refused).
    pub fn refuse(&self, path: &str, status: u16) {
        lock(&self.shared.state)
            .statuses
            .insert(path.to_owned(), status);
    }

    /// The API behind `path` is not switched on in the Cloud project: `403 accessNotConfigured`.
    pub fn disable_api(&self, path: &str) {
        lock(&self.shared.state).disabled.push(path.to_owned());
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.shared.hits.all()
    }
}

/// `spec` with every Google API row keeping its path and moving to the origin `base`.
fn point_at(base: &str, spec: &ProviderSpec) -> ProviderSpec {
    let mut out = spec.clone();
    for row in out.capabilities.iter_mut().filter(|row| {
        matches!(
            row.family,
            Family::GoogleCalendar
                | Family::GooglePeople
                | Family::GoogleTasks
                | Family::GoogleDrive
                | Family::GooglePhotosUpload
                | Family::GooglePhotosPicker
        )
    }) {
        if let Some(endpoint) = &row.endpoint {
            let path = endpoint
                .0
                .split_once("://")
                .and_then(|(_, rest)| rest.find('/').map(|at| &rest[at..]))
                .unwrap_or("");
            row.endpoint = Some(Endpoint(format!("{base}{path}")));
        }
    }
    out
}

fn denied(status: u16, reason: &str) -> Response {
    Response::json(
        status,
        &json!({"error": {"code": status, "status": reason, "errors": [{"reason": reason}]}}),
    )
}

fn answer(shared: &Shared, request: &Request) -> Response {
    let path = request.path();
    let Some(any_of) = wants(request) else {
        return Response::new(404);
    };
    let mut state = lock(&shared.state);
    let presented = request.bearer();
    let fixed = presented.is_some_and(|token| state.fixed.iter().any(|f| f == token));
    let live = presented.filter(|token| {
        fixed
            || shared
                .issuer
                .as_ref()
                .is_some_and(|issuer| issuer.access_is_live(token))
    });
    let Some(token) = live else {
        return denied(401, "UNAUTHENTICATED");
    };
    if let Some((left, after)) = state.throttle {
        state.throttle = (left > 1).then_some((left - 1, after));
        return denied(429, "rateLimitExceeded").with_header("Retry-After", &after.to_string());
    }
    if state.disabled.iter().any(|p| p == path) {
        return denied(403, "accessNotConfigured");
    }
    // A token the issuer issued carries the scopes it was issued for; a planted one carries none
    // that were recorded, and is let through.
    let scoped_out = !fixed
        && shared
            .issuer
            .as_ref()
            .and_then(|issuer| issuer.access_scope(token))
            .is_some_and(|granted| {
                !granted
                    .split_whitespace()
                    .any(|scope| any_of.contains(&scope))
            });
    if scoped_out {
        return denied(403, "insufficientPermissions");
    }
    if let Some(status) = state.statuses.get(path) {
        return Response::json(*status, &json!({"error": {"code": status}}));
    }
    let state = &mut *state;
    let body = match path {
        "/v1/userinfo" => json!({
            "sub": "1001", "email": state.address, "email_verified": true, "name": state.name,
        }),
        "/calendar/v3/users/me/calendarList" => {
            json!({"kind": "calendar#calendarList", "items": [
                {"id": state.address, "summary": state.address, "primary": true}
            ]})
        }
        "/v1/contactGroups" => json!({"contactGroups": [], "totalItems": 0}),
        "/tasks/v1/users/@me/lists" => {
            json!({"kind": "tasks#taskLists", "items": [{"id": "list-1", "title": "My Tasks"}]})
        }
        _ => {
            let drive =
                drive_routes::answer(&mut state.drive, state.drive_knobs, &shared.base, request);
            let other = || photos::answer(&mut state.photos, &shared.base, request);
            return drive.or_else(other).unwrap_or_else(|| Response::new(404));
        }
    };
    Response::json(200, &body)
}

impl FakeServer for FakeGoogle {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Google
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    /// Every Google API row keeps its path and moves to this server's origin.
    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        point_at(&self.shared.base, spec)
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let shared = self.shared;
        serve(
            self.listener,
            None,
            Arc::new(move |request: Request| {
                let response = answer(&shared, &request);
                shared.hits.push(Hit::of(&request, &response));
                response
            }),
        )
    }
}

/// A whole fake Google: the issuer (Google's style, requiring the application secret `secret`)
/// and the account APIs that accept its tokens.
#[derive(Debug)]
pub struct Google {
    /// The issuer: authorize, token, revoke.
    pub issuer: Running<IssuerHandle>,
    /// The account APIs.
    pub api: Running<GoogleHandle>,
}

impl Google {
    /// Starts both, the issuer requiring `secret` at its token endpoint.
    pub async fn start(secret: &str) -> io::Result<Self> {
        let issuer = FakeIssuer::start().await?;
        issuer.set_style(Style::Google);
        issuer.require_client_secret(secret);
        let api = FakeGoogle::start(&issuer).await?;
        Ok(Self { issuer, api })
    }
}
