//! A fake Nextcloud: Login Flow v2, the OCS app-password DELETE and capabilities, WebDAV with
//! sync tokens and quota, CalDAV and CardDAV collections, and the Notes API, behind HTTP Basic
//! with the app passwords the login flow hands out.

mod login;
mod notes;

use crate::dav::{Quota, Tree};
use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen, lock};
use crate::shipped::point;
use login::Logins;
use notes::Notes;
use porter_core::Family;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

pub use login::LoginPolicy;

const CAPABILITIES: &str = include_str!("../../fixtures/ocs_capabilities.json");

#[derive(Debug)]
struct State {
    user: String,
    app_passwords: Vec<String>,
    logins: Logins,
    tree: Tree,
    notes: Notes,
}

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    state: Arc<Mutex<State>>,
    hits: Seen<Hit>,
}

/// The test's side of a running Nextcloud.
#[derive(Debug, Clone)]
pub struct NextcloudHandle {
    shared: Shared,
}

/// The fake Nextcloud, bound and ready to serve.
#[derive(Debug)]
pub struct FakeNextcloud {
    listener: Listener,
    shared: Shared,
}

impl FakeNextcloud {
    /// Binds a Nextcloud with one user, over plain HTTP on loopback (Nextcloud's URLs are
    /// absolute, so a Unix socket will not do).
    pub async fn bind(user: &str) -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "nextcloud").await?;
        let port = crate::net::port_of(listener.address());
        let state = State {
            user: user.to_owned(),
            app_passwords: Vec::new(),
            logins: Logins::default(),
            tree: Tree::nextcloud(user),
            notes: Notes::default(),
        };
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                state: Arc::new(Mutex::new(state)),
                hits: Seen::default(),
            },
        })
    }

    /// The handle onto this server.
    pub fn handle(&self) -> NextcloudHandle {
        NextcloudHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start(user: &str) -> io::Result<Running<NextcloudHandle>> {
        let fake = Self::bind(user).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl NextcloudHandle {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// The user.
    pub fn user(&self) -> String {
        lock(&self.shared.state).user.clone()
    }

    /// What the person does when the login page is opened.
    pub fn set_login_policy(&self, policy: LoginPolicy) {
        lock(&self.shared.state).logins.policy = policy;
    }

    /// The person approves the login flow with this token (from the `login` URL).
    pub fn approve_login(&self, token: &str) {
        let mut state = lock(&self.shared.state);
        let State {
            logins,
            app_passwords,
            ..
        } = &mut *state;
        logins.approve(token, app_passwords);
    }

    /// The app passwords that work now.
    pub fn app_passwords(&self) -> Vec<String> {
        lock(&self.shared.state).app_passwords.clone()
    }

    /// Adds an app password (a seeded sign-in).
    pub fn seed_app_password(&self, password: &str) {
        lock(&self.shared.state)
            .app_passwords
            .push(password.to_owned());
    }

    /// Creates or replaces a file under the user's files, `rel` like `docs/a.txt`.
    pub fn put_file(&self, rel: &str, body: &[u8]) {
        let mut state = lock(&self.shared.state);
        let path = format!("/remote.php/dav/files/{}/{rel}", state.user);
        state
            .tree
            .put(&path, body)
            .expect("the parent folder exists");
    }

    /// Makes a folder under the user's files.
    pub fn mkdir(&self, rel: &str) {
        let mut state = lock(&self.shared.state);
        let path = format!("/remote.php/dav/files/{}/{rel}", state.user);
        state
            .tree
            .mkcol(&path)
            .expect("the parent folder exists and the folder is new");
    }

    /// Removes a file or folder under the user's files.
    pub fn delete_file(&self, rel: &str) {
        let mut state = lock(&self.shared.state);
        let path = format!("/remote.php/dav/files/{}/{rel}", state.user);
        state.tree.delete(&path);
    }

    /// Puts an item (an event, a task, a contact) into a collection (`personal`, `tasks`,
    /// `contacts`).
    pub fn put_item(&self, collection: &str, name: &str, body: &str) {
        let mut state = lock(&self.shared.state);
        let user = state.user.clone();
        let path = match collection {
            "contacts" => format!("/remote.php/dav/addressbooks/users/{user}/contacts/{name}"),
            other => format!("/remote.php/dav/calendars/{user}/{other}/{name}"),
        };
        state
            .tree
            .put(&path, body.as_bytes())
            .expect("the collection exists");
    }

    /// What quota reports.
    pub fn set_quota(&self, used_base: u64, available: i64) {
        lock(&self.shared.state).tree.set_quota(Quota {
            used_base,
            available,
        });
    }

    /// The sync token a client would get now.
    pub fn sync_token(&self) -> String {
        lock(&self.shared.state).tree.sync_token()
    }

    /// Sync tokens handed out so far are no longer valid.
    pub fn expire_sync_tokens(&self) {
        lock(&self.shared.state).tree.expire_sync_tokens();
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.shared.hits.all()
    }
}

impl FakeServer for FakeNextcloud {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Nextcloud
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        let user = lock(&self.shared.state).user.clone();
        let base = &self.shared.base;
        let dav = format!("{base}/remote.php/dav");
        let rows = [
            (Family::WebDav, format!("{dav}/files/{user}/")),
            (Family::CalDav, format!("{dav}/")),
            (Family::CardDav, format!("{dav}/")),
            (
                Family::NextcloudNotes,
                format!("{base}/index.php/apps/notes/api/v1/"),
            ),
        ];
        rows.iter().fold(spec.clone(), |spec, (family, url)| {
            point(&spec, *family, url)
        })
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let shared = self.shared;
        serve(
            self.listener,
            None,
            Arc::new(move |request| {
                let response = route(&shared, &request);
                shared.hits.push(Hit::of(&request, &response));
                response
            }),
        )
    }
}

fn ocs(status: u16, data: &serde_json::Value) -> Response {
    let meta = serde_json::json!({ "status": "ok", "statuscode": status, "message": "OK" });
    Response::json(
        status,
        &serde_json::json!({ "ocs": { "meta": meta, "data": data } }),
    )
}

fn challenge() -> Response {
    Response::new(401).with_header("WWW-Authenticate", "Basic realm=\"Nextcloud\"")
}

fn route(shared: &Shared, request: &Request) -> Response {
    let mut state = lock(&shared.state);
    let path = request.path().to_owned();
    match (request.method.as_str(), path.as_str()) {
        ("GET", "/status.php") => Response::json(
            200,
            &serde_json::json!({ "installed": true, "maintenance": false, "version": "28.0.4.1", "productname": "Nextcloud" }),
        ),
        ("GET", "/ocs/v2.php/cloud/capabilities") => {
            Response::new(200).typed("application/json", CAPABILITIES)
        }
        _ if path.starts_with("/index.php/login/v2") => {
            let State {
                logins,
                app_passwords,
                user,
                ..
            } = &mut *state;
            logins.route(request, &shared.base, user, app_passwords)
        }
        _ => {
            let authed = request
                .basic()
                .filter(|(u, p)| *u == state.user && state.app_passwords.contains(p));
            let Some((_, password)) = authed else {
                return challenge();
            };
            match (request.method.as_str(), path.as_str()) {
                ("DELETE", "/ocs/v2.php/core/apppassword") => {
                    state.app_passwords.retain(|p| *p != password);
                    ocs(200, &serde_json::json!([]))
                }
                (_, p) if p.starts_with("/remote.php/dav") => {
                    let p = p.trim_end_matches('/').to_owned();
                    state.tree.handle(request, &p)
                }
                (_, p) if p.starts_with("/index.php/apps/notes/api/v1/notes") => {
                    state.notes.route(request)
                }
                _ => Response::new(404),
            }
        }
    }
}
