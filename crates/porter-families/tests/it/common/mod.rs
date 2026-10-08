//! What the family tests share: an HTTP client that sends only to the fakes, and a person who
//! answers sheets.
#![allow(dead_code)]

#[cfg(feature = "generic")]
pub mod mail;

use porter_core::sheet::{
    FieldAnswer, FieldKind, FieldValue, ServiceChoice, SheetInput, SheetView,
};
use porter_core::{SecretText, Toggle};
use porter_http::{Http, HttpError, HttpRequest, HttpResponse, HyperHttp};
use porter_provider::{SignIn, SignInStep};
use std::sync::{Arc, Mutex};

/// One place a name is served: `host`'s paths under `prefix` go to `port` on loopback.
#[derive(Debug, Clone)]
pub struct Place {
    pub host: &'static str,
    pub prefix: &'static str,
    pub port: u16,
}

/// Sends `https://<host>/...` to the loopback port the table names for it, over plain HTTP, and
/// sends anything else nowhere (a family test never reaches the network). A request to a
/// loopback address goes straight to it.
#[derive(Debug, Clone)]
pub struct Fakes {
    pub places: Vec<Place>,
    pub inner: HyperHttp,
    pub sent: Arc<Mutex<Vec<String>>>,
}

impl Fakes {
    pub fn new(places: Vec<Place>) -> Self {
        Self {
            places,
            inner: HyperHttp::new(),
            sent: Arc::default(),
        }
    }

    /// Only loopback addresses are reachable.
    pub fn loopback() -> Self {
        Self::new(Vec::new())
    }
}

impl Http for Fakes {
    async fn send(&self, mut request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let origin = request.url.origin();
        self.sent
            .lock()
            .expect("sent")
            .push(format!("{} {}", request.method.token(), request.url));
        if !origin.is_loopback() {
            let place = self
                .places
                .iter()
                .filter(|p| p.host == origin.host && request.url.path().starts_with(p.prefix))
                .max_by_key(|p| p.prefix.len())
                .ok_or(HttpError::Unreachable)?;
            request.url = porter_core::WebUrl::parse(&format!(
                "http://127.0.0.1:{}{}",
                place.port,
                request.url.path()
            ))
            .map_err(|_| HttpError::Malformed)?;
        }
        self.inner.send(request).await
    }
}

/// Waits a few milliseconds whatever it is asked, so a poll loop does not spin while a person
/// takes their time.
#[derive(Debug, Clone, Copy)]
pub struct Brief;

impl porter_http::Sleep for Brief {
    async fn sleep(&self, _how_long: std::time::Duration) {
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }
}

pub fn plain(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Plain(text.to_owned()),
    }
}

pub fn secret(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Secret(SecretText::new(text)),
    }
}

/// Every service of a review, on.
pub fn all_on(kinds: impl IntoIterator<Item = porter_core::CapabilityKind>) -> Vec<ServiceChoice> {
    kinds
        .into_iter()
        .map(|kind| ServiceChoice {
            kind,
            toggle: Toggle::On,
        })
        .collect()
}

/// Feeds a sign-in `Start`, then what `answer` says to each step, until it is done or failed;
/// every step in order.
pub async fn drive<S: SignIn>(
    signin: &mut S,
    mut answer: impl FnMut(&SignInStep) -> porter_core::sheet::SignInInput,
) -> Vec<SignInStep> {
    use porter_core::sheet::SignInInput;
    let mut steps = Vec::new();
    let mut input = SignInInput::Start;
    for _ in 0..500 {
        let step = signin.next(input).await;
        let over = matches!(step, SignInStep::Done(_) | SignInStep::Failed(_));
        input = answer(&step);
        steps.push(step);
        if over {
            return steps;
        }
    }
    panic!("the sign-in did not end: {steps:#?}");
}

// ---- a person at the sheets, and a service to hand them to ----

use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::wire::ParentWindow;
use porter_core::{AccountId, CapabilityKind, Credential, SecretKey};
use porter_fake::{FixedClock, MemoryStore, RecordingAudit, ScriptedSheets};
use porter_families::FamilyProvider;
use porter_secrets::{MemorySecrets, Secrets, SecretsError};
use porter_service::{AccountService, SheetFault, SheetLink, SheetOpen, Sheets};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What the person at the sheet does.
#[derive(Debug, Clone, Default)]
pub struct Person {
    /// The provider they pick from the list (the first when none).
    pub pick: Option<porter_core::ProviderId>,
    /// What they type into any form.
    pub answers: Vec<FieldAnswer>,
    /// The services they turn off on the review.
    pub off: Vec<CapabilityKind>,
    /// Whether they open the page a browser step shows, and after how long.
    pub opens_page_after: Option<std::time::Duration>,
    /// Whether they close the sheet when it waits for the browser.
    pub gives_up: bool,
}

/// Sheets whose conversations are answered by `Person` (one per conversation, in order) and
/// whose consent asks go to a script.
#[derive(Debug)]
pub struct TestSheets {
    people: Mutex<std::collections::VecDeque<Person>>,
    consent: ScriptedSheets,
    shown: Arc<Mutex<Vec<SheetView>>>,
}

impl TestSheets {
    pub fn new(people: Vec<Person>, consent: ScriptedSheets) -> Self {
        Self {
            people: Mutex::new(people.into()),
            consent,
            shown: Arc::default(),
        }
    }

    pub fn shown(&self) -> Arc<Mutex<Vec<SheetView>>> {
        Arc::clone(&self.shown)
    }
}

#[derive(Debug)]
pub struct TestLink {
    person: Person,
    tx: UnboundedSender<SheetInput>,
    rx: UnboundedReceiver<SheetInput>,
    shown: Arc<Mutex<Vec<SheetView>>>,
}

impl TestLink {
    fn react(&self, view: &SheetView) {
        self.shown.lock().expect("shown").push(view.clone());
        let input = match view {
            SheetView::Providers(rows) => self
                .person
                .pick
                .clone()
                .or_else(|| rows.first().map(|r| r.id.clone()))
                .map(SheetInput::Pick),
            SheetView::SignIn(_) => Some(SheetInput::Submit(self.person.answers.clone())),
            SheetView::Review(review) => Some(SheetInput::Confirm(
                review
                    .review
                    .services
                    .iter()
                    .map(|row| ServiceChoice {
                        kind: row.kind,
                        toggle: match self.person.off.contains(&row.kind) {
                            true => Toggle::Off,
                            false => Toggle::On,
                        },
                    })
                    .collect(),
            )),
            SheetView::BrowserWait { url, .. } => {
                match (self.person.gives_up, self.person.opens_page_after) {
                    (true, _) => Some(SheetInput::Dismiss),
                    (false, Some(after)) => {
                        let url = url.to_string();
                        tokio::spawn(async move {
                            tokio::time::sleep(after).await;
                            open_page(&url).await;
                        });
                        None
                    }
                    (false, None) => None,
                }
            }
            // A sign-in that failed: the person reads it and closes the sheet.
            SheetView::Failed { .. } => Some(SheetInput::Dismiss),
            _ => None,
        };
        if let Some(input) = input {
            let _ = self.tx.send(input);
        }
    }
}

/// The person opens a page on a fake (a loopback address).
pub async fn open_page(url: &str) {
    use porter_fake_servers::browser::split_loopback;
    use porter_fake_servers::http::{Request, Scheme, send};
    let (address, target) = split_loopback(url).expect("a loopback page");
    send(&address, Scheme::Http, &Request::new("GET", &target))
        .await
        .expect("the page answers");
}

impl SheetLink for TestLink {
    async fn update(&mut self, view: SheetView) -> Result<(), SheetFault> {
        self.react(&view);
        Ok(())
    }

    async fn input(&mut self) -> Result<SheetInput, SheetFault> {
        self.rx.recv().await.ok_or(SheetFault::Closed)
    }
}

impl Sheets for TestSheets {
    type Link = TestLink;

    async fn consent(&self, ask: ConsentAsk, window: &ParentWindow) -> ConsentAnswer {
        self.consent.consent(ask, window).await
    }

    async fn conversation(&self, open: SheetOpen) -> Result<TestLink, SheetFault> {
        let person = self
            .people
            .lock()
            .expect("people")
            .pop_front()
            .ok_or(SheetFault::Unavailable)?;
        let (tx, rx) = unbounded_channel();
        let link = TestLink {
            person,
            tx,
            rx,
            shown: Arc::clone(&self.shown),
        };
        link.react(&open.view);
        Ok(link)
    }
}

/// A secret store the test keeps a handle on after the service has one.
#[derive(Debug, Clone, Default)]
pub struct SharedSecrets(pub Arc<MemorySecrets>);

impl SharedSecrets {
    pub async fn password(&self, account: &AccountId) -> Option<String> {
        let key = SecretKey {
            account: account.clone(),
            purpose: porter_core::SecretPurpose::Password,
        };
        match self.0.get(&key).await {
            Ok(Credential::Password(p)) => Some(p.expose().to_owned()),
            _ => None,
        }
    }
}

impl Secrets for SharedSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        self.0.put(key, value).await
    }
    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        self.0.get(key).await
    }
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.0.delete(key).await
    }
    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        self.0.delete_account(account).await
    }
}

pub type Service = AccountService<
    FamilyProvider,
    SharedSecrets,
    TestSheets,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

/// The service's seams, kept by the test.
pub struct Kept {
    pub secrets: SharedSecrets,
    pub store: MemoryStore,
    pub audit: RecordingAudit,
}

pub fn service(providers: Vec<FamilyProvider>, sheets: TestSheets) -> (Service, Kept) {
    let kept = Kept {
        secrets: SharedSecrets::default(),
        store: MemoryStore::default(),
        audit: RecordingAudit::default(),
    };
    let service = AccountService::new(
        providers,
        porter_service::Registry::default(),
        kept.secrets.clone(),
        sheets,
        FixedClock(porter_fake::NOW),
    )
    .with_store(kept.store.clone())
    .with_audit(kept.audit.clone());
    (service, kept)
}
