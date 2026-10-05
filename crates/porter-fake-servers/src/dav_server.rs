//! A plain DAV server (no Nextcloud around it): files, a calendar and an address book under
//! `/dav/`, behind HTTP Basic with one fixed user and password.

use crate::dav::{Kind, Quota, Tree};
use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen, lock};
use crate::shipped::point;
use porter_core::Family;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    login: (String, String),
    tree: Arc<Mutex<Tree>>,
    hits: Seen<Hit>,
}

/// The test's side of a running DAV server.
#[derive(Debug, Clone)]
pub struct DavHandle {
    shared: Shared,
}

/// The fake DAV server, bound and ready to serve.
#[derive(Debug)]
pub struct FakeDav {
    listener: Listener,
    shared: Shared,
}

impl FakeDav {
    /// Binds a server accepting `user` and `password`, over plain HTTP on loopback.
    pub async fn bind(user: &str, password: &str) -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "dav").await?;
        let port = crate::net::port_of(listener.address());
        let tree = Tree::with_collections(&[
            ("/dav", Kind::Container),
            ("/dav/files", Kind::Files),
            ("/dav/calendar", Kind::Calendar("VEVENT".to_owned())),
            ("/dav/contacts", Kind::AddressBook),
        ]);
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                login: (user.to_owned(), password.to_owned()),
                tree: Arc::new(Mutex::new(tree)),
                hits: Seen::default(),
            },
        })
    }

    /// The handle onto this server.
    pub fn handle(&self) -> DavHandle {
        DavHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start(user: &str, password: &str) -> io::Result<Running<DavHandle>> {
        let fake = Self::bind(user, password).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl DavHandle {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// Creates or replaces a file under `/dav/files/`.
    pub fn put_file(&self, rel: &str, body: &[u8]) {
        lock(&self.shared.tree)
            .put(&format!("/dav/files/{rel}"), body)
            .expect("the parent folder exists");
    }

    /// Removes a file or folder under `/dav/files/`.
    pub fn delete_file(&self, rel: &str) {
        lock(&self.shared.tree).delete(&format!("/dav/files/{rel}"));
    }

    /// What quota reports.
    pub fn set_quota(&self, used_base: u64, available: i64) {
        lock(&self.shared.tree).set_quota(Quota {
            used_base,
            available,
        });
    }

    /// The sync token a client would get now.
    pub fn sync_token(&self) -> String {
        lock(&self.shared.tree).sync_token()
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.shared.hits.all()
    }
}

impl FakeServer for FakeDav {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Dav
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        let base = &self.shared.base;
        let rows = [
            (Family::WebDav, format!("{base}/dav/files/")),
            (Family::CalDav, format!("{base}/dav/calendar/")),
            (Family::CardDav, format!("{base}/dav/contacts/")),
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
                let response = answer(&shared, &request);
                shared.hits.push(Hit::of(&request, &response));
                response
            }),
        )
    }
}

fn answer(shared: &Shared, request: &Request) -> Response {
    match request.basic() {
        Some(login) if login == shared.login => {
            let path = request.path().trim_end_matches('/').to_owned();
            lock(&shared.tree).handle(request, &path)
        }
        _ => Response::new(401).with_header("WWW-Authenticate", "Basic realm=\"fake dav\""),
    }
}
