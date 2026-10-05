//! The rig the Microsoft tests share: a fake issuer on loopback, an in-process Graph, and an
//! `Http` that sends issuer URLs over the loopback and Graph URLs to the in-process table.

use porter_core::sheet::SignInInput;
use porter_core::{Audience, UnixSeconds};
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Scheme, send};
use porter_fake_servers::{FakeIssuer, IssuerHandle, Running, shipped};
use porter_families::{MicrosoftEnv, MicrosoftProvider, SignInFlow};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse, Status};
use porter_oauth::ClientRegistry;
use porter_provider::{
    ClientChannel, ClientEntry, ClientId, ClientsFile, Issuer, ProviderSpec, SignIn, SignInMode,
    SignInStart, SignInStep,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const GRAPH: &str = "https://graph.microsoft.com";
pub const CLIENT_ID: &str = "microsoft-client";

/// The shipped `providers/microsoft.toml`.
pub fn spec() -> ProviderSpec {
    shipped::parse(include_str!("../../../../providers/microsoft.toml"))
}

/// Graph's answers: a status per path (200 unless set), who `/me` is, and every call received.
#[derive(Debug)]
pub struct Graph {
    pub issuer: IssuerHandle,
    pub address: Mutex<String>,
    pub statuses: Mutex<HashMap<String, u16>>,
    pub calls: Mutex<Vec<(String, String)>>,
    pub down: Mutex<bool>,
}

impl Graph {
    pub fn refuse(&self, path: &str, status: u16) {
        self.statuses
            .lock()
            .unwrap()
            .insert(path.to_owned(), status);
    }

    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }

    fn answer(&self, request: &HttpRequest) -> HttpResponse {
        let path = request.url.path().to_owned();
        let bearer = request
            .headers
            .iter()
            .find(|h| h.name.as_str().eq_ignore_ascii_case("authorization"))
            .and_then(|h| h.value.0.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_owned();
        self.calls
            .lock()
            .unwrap()
            .push((path.clone(), bearer.clone()));
        let status = match self.issuer.access_is_live(&bearer) {
            false => 401,
            true => self
                .statuses
                .lock()
                .unwrap()
                .get(&path)
                .copied()
                .unwrap_or(200),
        };
        let body = match (status, path.as_str()) {
            (200, "/v1.0/me") => {
                serde_json::json!({"mail": null, "userPrincipalName": *self.address.lock().unwrap()})
            }
            (200, "/v1.0/me/drive") => serde_json::json!({"quota": {"total": 5_000_000u64}}),
            (200, _) => serde_json::json!({"value": []}),
            _ => serde_json::json!({"error": {"code": "Forbidden"}}),
        };
        HttpResponse {
            status: Status(status),
            headers: vec![Header::new("Content-Type", "application/json")],
            body: body.to_string().into_bytes(),
        }
    }
}

/// Issuer URLs over loopback, Graph URLs to the table.
#[derive(Debug, Clone)]
pub struct Wire {
    pub graph: Arc<Graph>,
}

impl Http for Wire {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        if *self.graph.down.lock().unwrap() {
            return Err(HttpError::Unreachable);
        }
        if request.url.as_str().starts_with(GRAPH) {
            return Ok(self.graph.answer(&request));
        }
        let (address, target) =
            split_loopback(request.url.as_str()).map_err(|_| HttpError::Unreachable)?;
        let mut wire = Request::new(request.method.token(), &target).with_body(request.body);
        for h in &request.headers {
            wire = wire.with_header(h.name.as_str(), &h.value.0);
        }
        let response = send(&address, Scheme::Http, &wire)
            .await
            .map_err(|_| HttpError::Unreachable)?;
        Ok(HttpResponse {
            status: Status(response.status),
            headers: response
                .headers
                .iter()
                .map(|(n, v)| Header::new(n, v.clone()))
                .collect(),
            body: response.body,
        })
    }
}

pub struct Rig {
    pub issuer: Running<IssuerHandle>,
    pub graph: Arc<Graph>,
    pub clock: Arc<AtomicI64>,
    pub provider: MicrosoftProvider<Wire>,
}

pub fn client(issuer: &IssuerHandle) -> ClientEntry {
    ClientEntry {
        issuer: Issuer::Microsoft,
        channel: ClientChannel::Development,
        client_id: ClientId(CLIENT_ID.into()),
        client_secret: None,
        endpoints: Some(issuer.endpoints()),
    }
}

impl Rig {
    pub async fn new(flow: SignInFlow, with_client: bool) -> Self {
        let issuer = FakeIssuer::start().await.expect("issuer");
        let graph = Arc::new(Graph {
            issuer: (*issuer).clone(),
            address: Mutex::new("ada@contoso.onmicrosoft.com".into()),
            statuses: Mutex::default(),
            calls: Mutex::default(),
            down: Mutex::new(false),
        });
        let clients = match with_client {
            true => vec![client(&issuer)],
            false => Vec::new(),
        };
        let clock = Arc::new(AtomicI64::new(1_000_000));
        let ticking = Arc::clone(&clock);
        let counter = Arc::new(AtomicI64::new(0));
        let env = MicrosoftEnv::new(
            Wire {
                graph: Arc::clone(&graph),
            },
            ClientRegistry::layered(ClientsFile { clients }, ClientsFile::default()),
        )
        .with_channel(ClientChannel::Development)
        .with_flow(flow)
        .with_poll_slice(Duration::from_millis(20))
        .with_clock(Arc::new(move || {
            UnixSeconds(ticking.load(Ordering::SeqCst))
        }))
        .with_random(Arc::new(move || {
            let n = u8::try_from(counter.fetch_add(1, Ordering::SeqCst) % 200).unwrap_or(0);
            Some(([n; 32], [n.wrapping_add(1); 16]))
        }));
        Self {
            issuer,
            graph,
            clock,
            provider: MicrosoftProvider::with_env(spec(), env),
        }
    }

    pub fn advance(&self, seconds: i64) {
        self.clock.fetch_add(seconds, Ordering::SeqCst);
    }
}

pub fn start() -> SignInStart {
    SignInStart {
        mode: SignInMode::Add,
    }
}

pub fn audience(text: &str) -> Audience {
    Audience(text.to_owned())
}

/// The scripted browser: opens the launcher page, then follows what it redirects to.
pub async fn browse(page: &str) -> std::io::Result<porter_fake_servers::Visit> {
    let (address, target) = split_loopback(page)?;
    let first = send(&address, Scheme::Http, &Request::new("GET", &target)).await?;
    let location = first
        .header("location")
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("the launcher did not redirect"))?;
    porter_fake_servers::follow(&location).await
}

/// Feeds `Poll` until the sign-in says something other than `Waiting` (at most `limit` times).
pub async fn until_settled<S: SignIn>(signin: &mut S, limit: usize) -> SignInStep {
    for _ in 0..limit {
        match signin.next(SignInInput::Poll).await {
            SignInStep::Waiting => {}
            other => return other,
        }
    }
    SignInStep::Waiting
}
