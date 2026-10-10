//! The rig the Google tests share: a fake Google (its issuer in Google's style and its account
//! APIs) on loopback, the shipped provider file pointed at it, and an `Http` that sends every
//! URL the family builds to the loopback fake it names.

use porter_core::sheet::SignInInput;
use porter_core::{Audience, UnixSeconds};
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Scheme, send};
use porter_fake_servers::{Google, shipped};
use porter_families::{GoogleEnv, GoogleProvider};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse, Status};
use porter_oauth::{AppReview, ClientRegistry, ClientTraits, MailRights};
use porter_provider::{
    ClientChannel, ClientEntry, ClientId, ClientsFile, Issuer, ProviderSpec, SignIn, SignInMode,
    SignInStart, SignInStep,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const CLIENT_ID: &str = "google-test-client.apps.googleusercontent.com";
pub const SECRET: &str = "GOCSPX-test-application-secret";

/// Everything goes to the loopback fake the URL names; the `scope` of every refresh is noted.
#[derive(Debug, Clone, Default)]
pub struct Wire {
    refresh_scopes: Arc<Mutex<Vec<Option<String>>>>,
}

impl Wire {
    /// The `scope` of every refresh sent, in order (`None`: no scope named, the whole grant).
    pub fn refresh_scopes(&self) -> Vec<Option<String>> {
        self.refresh_scopes.lock().unwrap().clone()
    }
}

impl Http for Wire {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let (address, target) =
            split_loopback(request.url.as_str()).map_err(|_| HttpError::Unreachable)?;
        let mut wire = Request::new(request.method.token(), &target).with_body(request.body);
        for h in &request.headers {
            wire = wire.with_header(h.name.as_str(), &h.value.0);
        }
        if wire.form_value("grant_type").as_deref() == Some("refresh_token") {
            self.refresh_scopes
                .lock()
                .unwrap()
                .push(wire.form_value("scope"));
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

/// What the person's clients.toml row says.
#[derive(Debug, Clone, Copy)]
pub enum Row {
    /// No row: the owner has not registered a client.
    Absent,
    /// A row with these traits.
    Of(ClientTraits),
}

pub const PLAIN: Row = Row::Of(ClientTraits {
    mail: MailRights::Withheld,
    review: AppReview::Verified,
});
pub const TESTING: Row = Row::Of(ClientTraits {
    mail: MailRights::Withheld,
    review: AppReview::Testing,
});
pub const BYO: Row = Row::Of(ClientTraits {
    mail: MailRights::Byo,
    review: AppReview::Testing,
});

pub struct Rig {
    pub google: Google,
    pub clock: Arc<AtomicI64>,
    pub provider: GoogleProvider<Wire>,
    pub spec: ProviderSpec,
    pub wire: Wire,
}

pub fn entry(google: &Google) -> ClientEntry {
    ClientEntry {
        issuer: Issuer::Google,
        channel: ClientChannel::Development,
        client_id: ClientId(CLIENT_ID.into()),
        client_secret: Some(porter_core::SecretText::new(SECRET)),
        endpoints: Some(google.issuer.endpoints()),
    }
}

impl Rig {
    pub async fn new(row: Row) -> Self {
        let google = Google::start(SECRET).await.expect("fake google");
        let spec = google.api.rewrite(&shipped::google());
        let handle = google.api.clone();
        let registry = match row {
            Row::Absent => ClientRegistry::default(),
            Row::Of(traits) => ClientRegistry::layered(
                ClientsFile {
                    clients: vec![entry(&google)],
                },
                ClientsFile::default(),
            )
            .with_traits(Issuer::Google, ClientChannel::Development, traits),
        };
        let clock = Arc::new(AtomicI64::new(1_000_000));
        let ticking = Arc::clone(&clock);
        let counter = Arc::new(AtomicI64::new(0));
        let wire = Wire::default();
        // The clock is replaced below by the counting one.
        let env = GoogleEnv::new(
            wire.clone(),
            registry,
            porter_core::clock::FixedClock::new(UnixSeconds(0)),
        )
        .with_channel(ClientChannel::Development)
        .with_poll_slice(Duration::from_millis(20))
        .with_userinfo(
            porter_core::EndpointUrl::parse(&handle.userinfo_url()).expect("userinfo url"),
        )
        .with_clock(Arc::new(move || {
            UnixSeconds(ticking.load(Ordering::SeqCst))
        }))
        .with_random(Arc::new(move || {
            let n = u8::try_from(counter.fetch_add(1, Ordering::SeqCst) % 200).unwrap_or(0);
            Some(([n; 32], [n.wrapping_add(1); 16]))
        }));
        let provider = GoogleProvider::with_env(spec.clone(), env);
        Self {
            google,
            clock,
            provider,
            spec,
            wire,
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
