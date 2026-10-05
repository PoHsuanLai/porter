//! Autoconfig and well-known answers over HTTP, and a fake DNS answer table behind the `Dns`
//! seam `porter-discover` froze. The HTTP side serves a table of routes (a body or a redirect),
//! so a test says exactly which discovery URLs exist; any other path is a 404.

use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen, lock};
use porter_discover::{Dns, DnsFault, MxRecord, SrvRecord};
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::{DomainName, ProviderSpec};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

/// What a route answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `200` with this content type and body.
    Body {
        /// The content type.
        content_type: String,
        /// The body.
        body: String,
    },
    /// `301` to this location.
    Redirect(String),
}

/// A Mozilla autoconfig document for one domain, as providers publish it.
pub fn autoconfig_xml(domain: &str, imap: (&str, u16), smtp: (&str, u16)) -> String {
    format!(
        "<?xml version=\"1.0\"?><clientConfig version=\"1.1\"><emailProvider id=\"{domain}\">\
         <domain>{domain}</domain><displayName>Fake {domain}</displayName>\
         <incomingServer type=\"imap\"><hostname>{}</hostname><port>{}</port><socketType>SSL</socketType>\
         <authentication>password-cleartext</authentication><username>%EMAILADDRESS%</username></incomingServer>\
         <outgoingServer type=\"smtp\"><hostname>{}</hostname><port>{}</port><socketType>STARTTLS</socketType>\
         <authentication>password-cleartext</authentication><username>%EMAILADDRESS%</username></outgoingServer>\
         </emailProvider></clientConfig>",
        imap.0, imap.1, smtp.0, smtp.1
    )
}

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    hits: Seen<Hit>,
}

/// The test's side of a running autoconfig server.
#[derive(Debug, Clone)]
pub struct AutoconfigHandle {
    shared: Shared,
}

/// The fake autoconfig and well-known server.
#[derive(Debug)]
pub struct FakeAutoconfig {
    listener: Listener,
    shared: Shared,
}

impl FakeAutoconfig {
    /// Binds a server with no routes, on loopback.
    pub async fn bind() -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "autoconfig").await?;
        let port = crate::net::port_of(listener.address());
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                routes: Arc::default(),
                hits: Seen::default(),
            },
        })
    }

    /// The handle onto this server.
    pub fn handle(&self) -> AutoconfigHandle {
        AutoconfigHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start() -> io::Result<Running<AutoconfigHandle>> {
        let fake = Self::bind().await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl AutoconfigHandle {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// Serves `body` at `path` (no query).
    pub fn serve_body(&self, path: &str, content_type: &str, body: &str) {
        let route = Route::Body {
            content_type: content_type.to_owned(),
            body: body.to_owned(),
        };
        lock(&self.shared.routes).insert(path.to_owned(), route);
    }

    /// Redirects `path` to `location`.
    pub fn serve_redirect(&self, path: &str, location: &str) {
        lock(&self.shared.routes).insert(path.to_owned(), Route::Redirect(location.to_owned()));
    }

    /// Serves an autoconfig document at both places clients look: the `.well-known` path and
    /// `/mail/config-v1.1.xml`.
    pub fn serve_autoconfig(&self, xml: &str) {
        for path in [
            "/mail/config-v1.1.xml",
            "/.well-known/autoconfig/mail/config-v1.1.xml",
        ] {
            self.serve_body(path, "text/xml", xml);
        }
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.shared.hits.all()
    }
}

impl FakeServer for FakeAutoconfig {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Autoconfig
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    /// Discovery URLs are built from a domain, not read from the file: the spec is returned as is.
    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        spec.clone()
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
    match (
        request.method.as_str(),
        lock(&shared.routes).get(request.path()),
    ) {
        ("GET" | "HEAD", Some(Route::Body { content_type, body })) => {
            Response::new(200).typed(content_type, body.clone())
        }
        ("GET" | "HEAD", Some(Route::Redirect(to))) => {
            Response::new(301).with_header("Location", to)
        }
        _ => Response::new(404),
    }
}

/// A DNS answer table. Names without an entry have no records; `unreachable` makes every lookup
/// fail as a dead resolver does.
#[derive(Debug, Clone, Default)]
pub struct FakeDns {
    srv: Arc<Mutex<HashMap<String, Vec<SrvRecord>>>>,
    mx: Arc<Mutex<HashMap<String, Vec<MxRecord>>>>,
    dead: Arc<Mutex<bool>>,
    asked: Seen<String>,
}

impl FakeDns {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers SRV lookups of `name` with `records`.
    pub fn with_srv(self, name: &str, records: Vec<SrvRecord>) -> Self {
        lock(&self.srv).insert(name.to_ascii_lowercase(), records);
        self
    }

    /// Answers MX lookups of `domain` with `records`.
    pub fn with_mx(self, domain: &str, records: Vec<MxRecord>) -> Self {
        lock(&self.mx).insert(domain.to_ascii_lowercase(), records);
        self
    }

    /// Makes every lookup fail with `DnsFault::Unreachable`.
    pub fn unreachable(self) -> Self {
        *lock(&self.dead) = true;
        self
    }

    /// The names looked up, as `SRV name` and `MX domain`, in order.
    pub fn asked(&self) -> Vec<String> {
        self.asked.all()
    }

    fn answer<T: Clone>(
        &self,
        table: &Mutex<HashMap<String, Vec<T>>>,
        key: &str,
    ) -> Result<Vec<T>, DnsFault> {
        if *lock(&self.dead) {
            return Err(DnsFault::Unreachable);
        }
        match lock(table).get(&key.to_ascii_lowercase()) {
            Some(records) if !records.is_empty() => Ok(records.clone()),
            _ => Err(DnsFault::NoRecords),
        }
    }
}

impl Dns for FakeDns {
    fn srv(&self, name: &str) -> impl Future<Output = Result<Vec<SrvRecord>, DnsFault>> + Send {
        self.asked.push(format!("SRV {name}"));
        let answer = self.answer(&self.srv, name);
        async move { answer }
    }

    fn mx(
        &self,
        domain: &DomainName,
    ) -> impl Future<Output = Result<Vec<MxRecord>, DnsFault>> + Send {
        self.asked.push(format!("MX {}", domain.as_str()));
        let answer = self.answer(&self.mx, domain.as_str());
        async move { answer }
    }
}
