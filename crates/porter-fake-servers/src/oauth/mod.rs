//! A fake OAuth issuer: authorize (PKCE S256 required, state echoed), token (code exchange,
//! refresh with rotation, device code), device authorization, revoke and `invalid_grant`.
//! It speaks plain HTTP on loopback; the person's consent is a knob on the handle.

mod routes;

use crate::http::{Request, Response, percent_encode, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen, lock};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use porter_core::EndpointUrl;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::{IssuerEndpoints, ProviderSpec};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// What the person does on the authorize page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// Allows: the redirect carries a code.
    Grant,
    /// Denies: the redirect carries `error=access_denied`.
    Deny,
}

/// How the person answers a device code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceState {
    Pending,
    Approved,
    Denied,
}

/// What a token request got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenResult {
    /// Tokens were issued.
    Issued {
        /// The access token.
        access: String,
        /// The refresh token (the rotated one for a refresh).
        refresh: String,
    },
    /// The request was refused with this OAuth error.
    Rejected(String),
}

/// One thing the issuer received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssuerEvent {
    /// The authorize page was opened.
    Authorize {
        /// The client.
        client_id: String,
        /// Where the browser was sent back to.
        redirect_uri: String,
        /// The state the client sent.
        state: Option<String>,
        /// What the person did.
        consent: Consent,
    },
    /// An authorize request was refused (no PKCE S256, no redirect, ...).
    AuthorizeRefused {
        /// Why.
        error: String,
    },
    /// A token request.
    Token {
        /// The `grant_type`.
        grant_type: String,
        /// The client.
        client_id: String,
        /// What it got.
        result: TokenResult,
    },
    /// A revoke request.
    Revoke {
        /// The token presented.
        token: String,
        /// Whether the issuer knew it.
        known: bool,
    },
    /// A device code was requested.
    DeviceCode {
        /// The client.
        client_id: String,
    },
}

#[derive(Debug)]
struct Code {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    scope: String,
}

#[derive(Debug)]
struct Device {
    client_id: String,
    scope: String,
    state: DeviceState,
}

#[derive(Debug)]
struct Grant {
    client_id: String,
    scope: String,
}

#[derive(Debug)]
struct State {
    counter: u64,
    consent: Consent,
    refuse_refreshes: u32,
    codes: HashMap<String, Code>,
    refresh: HashMap<String, Grant>,
    access: HashMap<String, String>,
    devices: HashMap<String, Device>,
}

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    state: Arc<Mutex<State>>,
    events: Seen<IssuerEvent>,
}

/// The test's side of a running issuer: what it saw and the knobs.
#[derive(Debug, Clone)]
pub struct IssuerHandle {
    shared: Shared,
}

/// The fake issuer, bound and ready to serve.
#[derive(Debug)]
pub struct FakeIssuer {
    listener: Listener,
    shared: Shared,
}

impl FakeIssuer {
    /// Binds on loopback (an issuer's redirects are absolute URLs, so a Unix socket will not do).
    pub async fn bind() -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "oauth").await?;
        let port = crate::net::port_of(listener.address());
        let state = State {
            counter: 0,
            consent: Consent::Grant,
            refuse_refreshes: 0,
            codes: HashMap::new(),
            refresh: HashMap::new(),
            access: HashMap::new(),
            devices: HashMap::new(),
        };
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                state: Arc::new(Mutex::new(state)),
                events: Seen::default(),
            },
        })
    }

    /// The handle onto this issuer.
    pub fn handle(&self) -> IssuerHandle {
        IssuerHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start() -> io::Result<Running<IssuerHandle>> {
        let fake = Self::bind().await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl IssuerHandle {
    /// The issuer's origin, `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// The endpoints, as a client registration pointing at this issuer would carry them.
    pub fn endpoints(&self) -> IssuerEndpoints {
        let url = |path: &str| {
            EndpointUrl::parse(&format!("{}{path}", self.shared.base)).expect("a loopback URL")
        };
        IssuerEndpoints {
            authorize: url("/authorize"),
            token: url("/token"),
            revoke: Some(url("/revoke")),
            device: Some(url("/device")),
        }
    }

    /// What the person does from now on at the authorize page.
    pub fn set_consent(&self, consent: Consent) {
        lock(&self.shared.state).consent = consent;
    }

    /// The next `count` refresh requests get `invalid_grant`, as for a revoked or expired grant.
    pub fn refuse_refreshes(&self, count: u32) {
        lock(&self.shared.state).refuse_refreshes = count;
    }

    /// Issues a refresh token as if the person had already signed in (a seeded account).
    pub fn seed_refresh(&self, client_id: &str, scope: &str) -> String {
        let mut state = lock(&self.shared.state);
        let token = routes::next(&mut state, "fake-refresh");
        let grant = Grant {
            client_id: client_id.to_owned(),
            scope: scope.to_owned(),
        };
        state.refresh.insert(token.clone(), grant);
        token
    }

    /// Plants this exact refresh token as if the person had already signed in: the same as
    /// [`IssuerHandle::seed_refresh`] with a name the caller chose (a string a secret scan can
    /// look for).
    pub fn seed_refresh_as(&self, token: &str, client_id: &str, scope: &str) {
        let grant = Grant {
            client_id: client_id.to_owned(),
            scope: scope.to_owned(),
        };
        lock(&self.shared.state)
            .refresh
            .insert(token.to_owned(), grant);
    }

    /// Plants this exact access token as live (the issuer and every drive that asks it accept it
    /// until it is revoked).
    pub fn seed_access_as(&self, token: &str, client_id: &str) {
        lock(&self.shared.state)
            .access
            .insert(token.to_owned(), client_id.to_owned());
    }

    /// The person approves the device code with this user code.
    pub fn approve_device(&self, user_code: &str) {
        self.answer_device(user_code, DeviceState::Approved);
    }

    /// The person denies the device code with this user code.
    pub fn deny_device(&self, user_code: &str) {
        self.answer_device(user_code, DeviceState::Denied);
    }

    fn answer_device(&self, user_code: &str, to: DeviceState) {
        let mut state = lock(&self.shared.state);
        let code = format!(
            "fake-device-{}",
            user_code
                .trim_start_matches("FAKE-")
                .trim_start_matches('0')
        );
        if let Some(device) = state.devices.get_mut(&code) {
            device.state = to;
        }
    }

    /// Whether this refresh token is live (issued, not rotated away, not revoked).
    pub fn refresh_is_live(&self, token: &str) -> bool {
        lock(&self.shared.state).refresh.contains_key(token)
    }

    /// Whether this access token is live.
    pub fn access_is_live(&self, token: &str) -> bool {
        lock(&self.shared.state).access.contains_key(token)
    }

    /// Everything the issuer received, oldest first.
    pub fn events(&self) -> Vec<IssuerEvent> {
        self.shared.events.all()
    }
}

impl FakeServer for FakeIssuer {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::OAuthIssuer
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    /// An issuer is not a row of a provider file: the file names the issuer, and the client
    /// registration carries its endpoints (`IssuerHandle::endpoints`). The spec is returned as is.
    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        spec.clone()
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let shared = self.shared;
        serve(
            self.listener,
            None,
            Arc::new(move |request| routes::route(&shared, &request)),
        )
    }
}
