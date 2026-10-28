//! The rig the Microsoft tests share: a fake issuer on loopback, an in-process Graph, and an
//! `Http` that sends issuer URLs over the loopback and Graph URLs to the in-process table.

use porter_core::Audience;
use porter_core::sheet::SignInInput;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Scheme, send};
use porter_fake_servers::{FakeIssuer, IssuerHandle, Running, shipped};
use porter_families::{MicrosoftEnv, MicrosoftProvider};
use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use porter_ids::UnixSeconds;
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
    shipped::parse(include_str!("../../../../../providers/microsoft.toml"))
}

/// Graph's answers: a status per path (200 unless set), who `/me` is, and every call received.
#[derive(Debug)]
pub struct Graph {
    pub issuer: IssuerHandle,
    pub address: Mutex<String>,
    pub statuses: Mutex<HashMap<String, u16>>,
    pub calls: Mutex<Vec<(String, String)>>,
    pub down: Mutex<bool>,
    /// The `scope` of every refresh sent to the issuer, in order (`None`: no scope named).
    pub refresh_scopes: Mutex<Vec<Option<String>>>,
}

impl Graph {
    pub fn refresh_scopes(&self) -> Vec<Option<String>> {
        self.refresh_scopes.lock().unwrap().clone()
    }

    pub fn refuse(&self, path: &str, status: u16) {
        self.statuses
            .lock()
            .unwrap()
            .insert(path.to_owned(), status);
    }

    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }

    fn answer(&self, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        let path = request.uri().path().to_owned();
        let bearer = request
            .headers()
            .get(porter_http::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
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
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(body.to_string().into_bytes())
            .expect("response")
    }
}

/// Issuer URLs over loopback, Graph URLs to the table.
#[derive(Debug, Clone)]
pub struct Wire {
    pub graph: Arc<Graph>,
}

impl Http for Wire {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
        if *self.graph.down.lock().unwrap() {
            return Err(HttpError::Unreachable);
        }
        if request.uri().to_string().starts_with(GRAPH) {
            return Ok(self.graph.answer(&request));
        }
        let (address, target) =
            split_loopback(&request.uri().to_string()).map_err(|_| HttpError::Unreachable)?;
        let mut wire = porter_fake_servers::http::Request::new(request.method().as_str(), &target);
        for (name, value) in request.headers() {
            if let Ok(value) = value.to_str() {
                wire = wire.with_header(name.as_str(), value);
            }
        }
        let wire = wire.with_body(request.into_body());
        if wire.form_value("grant_type").as_deref() == Some("refresh_token") {
            self.graph
                .refresh_scopes
                .lock()
                .unwrap()
                .push(wire.form_value("scope"));
        }
        let response = send(&address, Scheme::Http, &wire)
            .await
            .map_err(|_| HttpError::Unreachable)?;
        let answer = response
            .headers
            .iter()
            .fold(Response::builder().status(response.status), |b, (n, v)| {
                b.header(n.as_str(), v.as_str())
            });
        Ok(answer.body(response.body)?)
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
    pub async fn new(with_client: bool) -> Self {
        Self::over(with_client, spec()).await
    }

    /// A rig whose provider file is `spec`.
    pub async fn over(with_client: bool, spec: ProviderSpec) -> Self {
        let issuer = FakeIssuer::start().await.expect("issuer");
        let graph = Arc::new(Graph {
            issuer: (*issuer).clone(),
            address: Mutex::new("ada@contoso.onmicrosoft.com".into()),
            statuses: Mutex::default(),
            calls: Mutex::default(),
            down: Mutex::new(false),
            refresh_scopes: Mutex::default(),
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
            // The clock is replaced below by the counting one.
            porter_ids::clock::FixedClock::new(UnixSeconds(0)),
        )
        .with_channel(ClientChannel::Development)
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
            provider: MicrosoftProvider::with_env(spec, env),
        }
    }

    pub fn advance(&self, seconds: i64) {
        self.clock.fetch_add(seconds, Ordering::SeqCst);
    }
}

pub fn start() -> SignInStart {
    SignInStart::new(SignInMode::Add)
}

pub fn audience(text: &str) -> Audience {
    Audience(text.to_owned())
}

/// The scripted browser: opens the authorize URL itself, which the fake issuer redirects to the
/// sign-in's loopback listener.
pub async fn browse(page: &str) -> std::io::Result<porter_fake_servers::Visit> {
    porter_fake_servers::follow(page).await
}

/// The port of the loopback listener an authorize URL asks the browser to come back to (its
/// percent-encoded `redirect_uri` parameter).
pub fn redirect_port(page: &str) -> Option<u16> {
    let query = page.split_once('?')?.1;
    let redirect = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("redirect_uri="))?;
    let decoded = redirect.replace("%3A", ":").replace("%2F", "/");
    let authority = decoded.strip_prefix("http://127.0.0.1:")?;
    authority.split('/').next()?.parse().ok()
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
