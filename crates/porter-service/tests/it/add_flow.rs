//! Adding an account and signing in again, over a provider whose sign-in is a script: the host
//! half of the conversation (what is drawn, what the sign-in is told, what is stored, when).

use porter_core::audit::AuditEvent;
use porter_core::capability::CapabilityKind;
use porter_core::capability::{
    Access, Capability, Delta, LabelModel, MailCap, MailTransport, Offered,
};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::sheet::{
    Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, ServiceChoice, SheetInput,
    SheetView, SignInFault, SignInInput, UserCode,
};
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AccountsReply, AccountsRequest, AppId, AppName,
    AuthKind, Claim, Credential, DataClass, EndpointUrl, Family, Isolation, LoginName, Offer,
    Provenance, ProviderId, Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint,
    SpaceScope, Subject, Tls, Toggle, UnixSeconds, WebUrl,
};
use porter_fake::{FixedClock, MemoryStore, RecordingAudit, ScriptedSheets};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignIn,
    SignInMode, SignInStart, SignInStep, Signed, parse_provider,
};
use porter_secrets::{MemorySecrets, Secrets, SecretsError};
use porter_service::{AccountService, Registry, SheetFault, SheetLink, SheetOpen, Sheets};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

// ---- a provider whose sign-in is a list of steps ----

const SPEC: &str = r#"
id = "scripted"
label = "Scripted"
mark = "generic"
[auth]
kind = "password"
[discovery]
kind = "fixed"
[[capability]]
family = "imap"
kind = "mail"
v = { access = "read_write", send = "present", delta = "poll", transport = "imap", labels = "folders" }
"#;

const LOCAL: &str = r#"
id = "runtime"
label = "Runtime"
mark = "generic"
[auth]
kind = "local_runtime"
[discovery]
kind = "supervised"
[ai]
locality = { kind = "on_device" }
billing = { kind = "free" }
[[capability]]
family = "ollama_native"
kind = "llm"
v = { features = ["chat"], context = 2048, max_output = 2048, wire = "ollama_native" }
"#;

/// What the sign-in was told, by name, and the steps it will answer with, in order.
#[derive(Debug, Clone, Default)]
struct Script {
    steps: Arc<Mutex<VecDeque<SignInStep>>>,
    told: Arc<Mutex<Vec<&'static str>>>,
    modes: Arc<Mutex<Vec<SignInMode>>>,
    /// The accounts the provider was asked to revoke, as it was handed them.
    revoked: Arc<Mutex<Vec<Account>>>,
    /// Whether the provider fails when asked to revoke.
    revoke_fails: Arc<Mutex<bool>>,
}

impl Script {
    fn answering(steps: Vec<SignInStep>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into())),
            ..Self::default()
        }
    }

    fn told(&self) -> Vec<&'static str> {
        self.told.lock().expect("told").clone()
    }
}

#[derive(Debug, Clone)]
struct ScriptedProvider {
    spec: ProviderSpec,
    script: Script,
    refuses_to_start: bool,
}

#[derive(Debug)]
struct ScriptedSignIn(Script);

#[derive(Debug)]
struct NoSession;

impl SignIn for ScriptedSignIn {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let name = match &input {
            SignInInput::Start => "start",
            SignInInput::Fields(_) => "fields",
            SignInInput::Confirm(_) => "confirm",
            SignInInput::Poll => "poll",
            SignInInput::Cancel => "cancel",
        };
        self.0.told.lock().expect("told").push(name);
        let next = self.0.steps.lock().expect("steps").pop_front();
        match (input, next) {
            (SignInInput::Cancel, _) => SignInStep::Failed(SignInFault::Cancelled),
            (_, Some(step)) => step,
            (_, None) => SignInStep::Waiting,
        }
    }
}

impl Provider for ScriptedProvider {
    type Session = NoSession;
    type SignIn = ScriptedSignIn;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        Ok(vec![])
    }

    async fn open(
        &self,
        _account: &AccountId,
        _presented: Presented,
    ) -> Result<NoSession, ProviderError> {
        Ok(NoSession)
    }

    fn sign_in(&self, start: SignInStart) -> Result<ScriptedSignIn, ProviderError> {
        self.script.modes.lock().expect("modes").push(start.mode);
        match self.refuses_to_start {
            true => Err(ProviderError::Forbidden),
            false => Ok(ScriptedSignIn(self.script.clone())),
        }
    }

    async fn revoke(
        &self,
        account: &Account,
        _presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        self.script
            .revoked
            .lock()
            .expect("revoked")
            .push(account.clone());
        match *self.script.revoke_fails.lock().expect("flag") {
            true => Err(ProviderError::Unreachable),
            false => Ok(RevokeOutcome::Revoked),
        }
    }
}

impl ProviderSession for NoSession {
    async fn access_token(
        &self,
        _audience: &porter_core::Audience,
    ) -> Result<porter_core::IssuedToken, ProviderError> {
        Err(ProviderError::Forbidden)
    }

    fn renewed(&self) -> Option<Credential> {
        None
    }
}

fn provider(script: &Script) -> ScriptedProvider {
    ScriptedProvider {
        spec: parse_provider(SPEC).expect("spec"),
        script: script.clone(),
        refuses_to_start: false,
    }
}

// ---- a person at the sheet ----

type Reactor = Arc<dyn Fn(&SheetView) -> Option<SheetInput> + Send + Sync>;

struct Reactive {
    people: Mutex<VecDeque<Reactor>>,
    shown: Arc<Mutex<Vec<SheetView>>>,
    consent: ScriptedSheets,
}

impl Reactive {
    fn new(people: Vec<Reactor>) -> Self {
        Self {
            people: Mutex::new(people.into()),
            shown: Arc::default(),
            consent: ScriptedSheets::default(),
        }
    }
}

struct Link {
    react: Reactor,
    tx: UnboundedSender<SheetInput>,
    rx: UnboundedReceiver<SheetInput>,
    shown: Arc<Mutex<Vec<SheetView>>>,
}

impl Link {
    fn see(&self, view: &SheetView) {
        self.shown.lock().expect("shown").push(view.clone());
        if let Some(input) = (self.react)(view) {
            let _ = self.tx.send(input);
        }
    }
}

impl SheetLink for Link {
    async fn update(&mut self, view: SheetView) -> Result<(), SheetFault> {
        self.see(&view);
        Ok(())
    }

    async fn input(&mut self) -> Result<SheetInput, SheetFault> {
        self.rx.recv().await.ok_or(SheetFault::Closed)
    }
}

impl Sheets for Reactive {
    type Link = Link;

    async fn consent(
        &self,
        ask: porter_core::consent::ConsentAsk,
        window: &ParentWindow,
    ) -> porter_core::consent::ConsentAnswer {
        self.consent.consent(ask, window).await
    }

    async fn conversation(&self, open: SheetOpen) -> Result<Link, SheetFault> {
        let react = self
            .people
            .lock()
            .expect("people")
            .pop_front()
            .ok_or(SheetFault::Unavailable)?;
        let (tx, rx) = unbounded_channel();
        let link = Link {
            react,
            tx,
            rx,
            shown: Arc::clone(&self.shown),
        };
        link.see(&open.view);
        Ok(link)
    }
}

/// The person who types an address, confirms a review, and closes the sheet on a failure.
fn typist() -> Reactor {
    Arc::new(|view| match view {
        SheetView::Providers(rows) => Some(SheetInput::Pick(rows[0].id.clone())),
        SheetView::SignIn(_) => Some(SheetInput::Submit(vec![FieldAnswer {
            kind: FieldKind::Address,
            value: FieldValue::Plain("ada@example.org".into()),
        }])),
        SheetView::Review(review) => Some(SheetInput::Confirm(
            review
                .review
                .services
                .iter()
                .map(|r| ServiceChoice {
                    kind: r.kind,
                    toggle: Toggle::On,
                })
                .collect(),
        )),
        SheetView::Failed { .. } => Some(SheetInput::Dismiss),
        _ => None,
    })
}

// ---- the service ----

#[derive(Debug, Clone, Default)]
struct Shared(Arc<MemorySecrets>);

impl Secrets for Shared {
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

/// A store that takes the first credential and refuses the second.
#[derive(Debug, Default)]
struct OneThenFull(Shared, Mutex<usize>);

impl Secrets for OneThenFull {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        let filed = {
            let mut count = self.1.lock().expect("count");
            *count += 1;
            *count
        };
        match filed {
            1 => self.0.put(key, value).await,
            _ => Err(SecretsError::Unavailable),
        }
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

struct Kept {
    secrets: Shared,
    store: MemoryStore,
    audit: RecordingAudit,
    shown: Arc<Mutex<Vec<SheetView>>>,
}

type Service<S = Shared> =
    AccountService<ScriptedProvider, S, Reactive, FixedClock, MemoryStore, RecordingAudit>;

fn service_over<S: Secrets>(
    providers: Vec<ScriptedProvider>,
    registry: Registry,
    secrets: S,
    people: Vec<Reactor>,
) -> (Service<S>, Kept) {
    let sheets = Reactive::new(people);
    let kept = Kept {
        secrets: Shared::default(),
        store: MemoryStore::default(),
        audit: RecordingAudit::default(),
        shown: Arc::clone(&sheets.shown),
    };
    let service = AccountService::new(
        providers,
        registry,
        secrets,
        sheets,
        FixedClock(UnixSeconds(1_790_000_000)),
    )
    .with_store(kept.store.clone())
    .with_audit(kept.audit.clone());
    (service, kept)
}

fn service(script: &Script, people: Vec<Reactor>) -> (Service, Kept) {
    let secrets = Shared::default();
    let (service, kept) = service_over(
        vec![provider(script)],
        Registry::default(),
        secrets.clone(),
        people,
    );
    (service, Kept { secrets, ..kept })
}

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    }
}

fn add_request() -> AccountsRequest {
    AccountsRequest::AddAccount {
        hint: ProviderHint::Any,
        window: ParentWindow::Unparented,
    }
}

// ---- what a sign-in says ----

fn imap_claim() -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(Capability::Mail(MailCap {
            access: Access::ReadWrite,
            send: Offered::Present,
            delta: Delta::Poll,
            transport: MailTransport::Imap,
            labels: LabelModel::Folders,
        })),
        provenance: Provenance::Discovered,
    }
}

fn imap(login: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        family: Family::Imap,
        url: EndpointUrl::parse("imaps://imap.example.org:993").expect("url"),
        tls: Tls::Implicit,
        login: LoginName(login.into()),
    }
}

fn signed(login: &str) -> Signed {
    Signed {
        label: AccountLabel(format!("{login}@example.org")),
        credentials: vec![
            (
                SecretPurpose::IncomingPassword,
                Credential::Password(SecretText::new("in-pw")),
            ),
            (
                SecretPurpose::OutgoingPassword,
                Credential::Password(SecretText::new("out-pw")),
            ),
        ],
        claims: vec![imap_claim()],
        endpoints: vec![imap(login)],
        restriction: Restriction::none(),
    }
}

fn ask() -> SignInStep {
    SignInStep::AskFields(vec![FieldSpec {
        kind: FieldKind::Address,
        entry: Entry::Plain,
        presence: Presence::Required,
        prefill: None,
    }])
}

fn review(login: &str) -> SignInStep {
    let signed = signed(login);
    SignInStep::Review {
        claims: signed.claims,
        endpoints: signed.endpoints,
        restriction: signed.restriction,
        label: signed.label,
    }
}

fn kinds(views: &Arc<Mutex<Vec<SheetView>>>) -> Vec<&'static str> {
    views
        .lock()
        .expect("shown")
        .iter()
        .map(|view| match view {
            SheetView::Consent(_) => "consent",
            SheetView::Providers(_) => "providers",
            SheetView::SignIn(_) => "form",
            SheetView::BrowserWait { .. } => "browser",
            SheetView::ShowCode { .. } => "code",
            SheetView::Review(_) => "review",
            SheetView::Working { .. } => "working",
            SheetView::Failed { .. } => "failed",
            SheetView::Done => "done",
        })
        .collect()
}

async fn added(service: &Service, caller: &AppId) -> AccountsReply {
    service.handle(caller, add_request()).await
}

#[tokio::test]
async fn a_walk_through_the_sheet_stores_the_account_its_secrets_and_its_audit_line() {
    let script = Script::answering(vec![ask(), review("ada"), SignInStep::Done(signed("ada"))]);
    let (service, kept) = service(&script, vec![typist()]);
    let reply = added(&service, &app("org.quire.Mail")).await;
    let AccountsReply::Added(id) = reply else {
        panic!("added: {reply:?}");
    };
    assert_eq!(id.as_str(), "scripted-ada-example.org");

    assert_eq!(
        kinds(&kept.shown),
        [
            "providers",
            "working",
            "form",
            "working",
            "review",
            "working",
            "done"
        ]
    );
    // The sign-in was told, in order: start, the answers, the confirmation.
    assert_eq!(script.told(), ["start", "fields", "confirm"]);

    let registry = service.registry();
    let account = &registry.accounts[0];
    assert_eq!(
        (
            account.id.clone(),
            account.provider.as_str(),
            account.auth,
            account.state
        ),
        (id.clone(), "scripted", AuthKind::Password, AccountState::Ok)
    );
    assert_eq!(account.label.0, "ada@example.org");
    assert_eq!(account.endpoints, vec![imap("ada")]);
    assert_eq!(account.capabilities, vec![imap_claim()]);
    assert_eq!(
        kept.store.stored().expect("saved").accounts,
        registry.accounts
    );

    // Each credential is filed under its own purpose; the audit line names no secret.
    for (purpose, want) in [
        (SecretPurpose::IncomingPassword, "in-pw"),
        (SecretPurpose::OutgoingPassword, "out-pw"),
    ] {
        let key = SecretKey {
            account: id.clone(),
            purpose,
        };
        assert_eq!(
            kept.secrets.get(&key).await,
            Ok(Credential::Password(SecretText::new(want)))
        );
    }
    let entries = kept.audit.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (
            entries[0].event.clone(),
            entries[0].app.clone(),
            entries[0].account.clone()
        ),
        (AuditEvent::SignedIn, Some(app("org.quire.Mail")), Some(id))
    );
    assert!(!format!("{entries:?}").contains("pw"));
}

#[tokio::test]
async fn a_service_turned_off_on_the_review_is_stored_off() {
    let script = Script::answering(vec![ask(), review("ada"), SignInStep::Done(signed("ada"))]);
    let off: Reactor = Arc::new(|view| match view {
        SheetView::Review(r) => Some(SheetInput::Confirm(
            r.review
                .services
                .iter()
                .map(|row| ServiceChoice {
                    kind: row.kind,
                    toggle: Toggle::Off,
                })
                .collect(),
        )),
        other => typist()(other),
    });
    let (service, _kept) = service(&script, vec![off]);
    let AccountsReply::Added(_) = added(&service, &app("org.quire.Mail")).await else {
        panic!("added");
    };
    let registry = service.registry();
    assert_eq!(registry.toggles.len(), 1);
    assert_eq!(registry.toggles[0].kind, CapabilityKind::Mail);
    assert!(matches!(
        registry.accounts[0].capabilities[0].offer,
        Offer::Absent { .. }
    ));
}

#[tokio::test]
async fn a_failed_sign_in_ends_with_the_refusal_the_fault_means() {
    let cases = [
        (SignInFault::Refused, Refusal::Denied),
        (SignInFault::Forbidden, Refusal::Denied),
        (SignInFault::Unreachable, Refusal::Unavailable),
        (SignInFault::Unreadable, Refusal::Unavailable),
        (SignInFault::TimedOut, Refusal::Unavailable),
        (SignInFault::NeedsClientId, Refusal::Unavailable),
    ];
    for (fault, refusal) in cases {
        let script = Script::answering(vec![ask(), SignInStep::Failed(fault)]);
        let (service, kept) = service(&script, vec![typist()]);
        assert_eq!(
            added(&service, &app("org.quire.Mail")).await,
            AccountsReply::Refused(refusal),
            "{fault:?}"
        );
        assert!(service.registry().accounts.is_empty());
        assert_eq!(kept.audit.entries(), vec![]);
    }
}

/// An agent that signs itself in (Claude Code), with a sign-in that would review and finish.
fn agent_service(script: &Script, launchers: &'static [&'static str]) -> (Service, Kept) {
    let spec = parse_provider(include_str!("../../../../providers/claude-code.toml"))
        .expect("the shipped file");
    let agent = ScriptedProvider {
        spec,
        script: script.clone(),
        refuses_to_start: false,
    };
    let (service, kept) = service_over(
        vec![agent],
        Registry::default(),
        Shared::default(),
        vec![typist(), typist()],
    );
    service.set_launcher_roster(porter_service::Roster::new(move |program| {
        launchers.contains(&program.as_str())
    }));
    (service, kept)
}

#[tokio::test]
async fn adding_an_agent_with_no_launcher_ends_no_launcher_and_stores_nothing() {
    let script = Script::answering(vec![review("claude"), SignInStep::Done(signed("claude"))]);
    let (service, kept) = agent_service(&script, &["codex"]);
    assert_eq!(
        added(&service, &app("org.quire.Mail")).await,
        AccountsReply::Refused(Refusal::NoLauncher)
    );
    assert!(service.registry().accounts.is_empty());
    assert_eq!(script.told(), Vec::<&str>::new(), "the sign-in never began");
    assert_eq!(kept.audit.entries(), vec![]);
    assert_eq!(kinds(&kept.shown), ["providers", "working", "failed"]);
}

#[tokio::test]
async fn adding_an_agent_with_a_launcher_makes_a_needs_login_account() {
    let script = Script::answering(vec![review("claude"), SignInStep::Done(signed("claude"))]);
    let (service, _kept) = agent_service(&script, &["claude-code"]);
    let reply = added(&service, &app("org.quire.Mail")).await;
    assert!(matches!(reply, AccountsReply::Added(_)), "{reply:?}");
    let registry = service.registry();
    assert_eq!(registry.accounts.len(), 1);
    assert_eq!(registry.accounts[0].state, AccountState::NeedsLogin);
}

#[tokio::test]
async fn the_same_agent_added_twice_is_already_added_whatever_its_sign_in_shows() {
    let script = Script::answering(vec![review("claude"), SignInStep::Done(signed("claude"))]);
    let (service, kept) = agent_service(&script, &["claude-code"]);
    let caller = app("org.quire.Mail");
    assert!(matches!(
        added(&service, &caller).await,
        AccountsReply::Added(_)
    ));
    let held = service.registry().accounts.clone();
    // Another name in the second sign-in: an agent is one account of its program however its
    // sign-in reads, so only the agent rule can say it is there already.
    script
        .steps
        .lock()
        .expect("steps")
        .extend([review("other"), SignInStep::Done(signed("other"))]);
    kept.shown.lock().expect("shown").clear();
    assert_eq!(
        added(&service, &caller).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    assert_eq!(kinds(&kept.shown).last(), Some(&"failed"));
    assert_eq!(service.registry().accounts, held);
}

#[tokio::test]
async fn the_same_login_added_twice_ends_at_the_review_and_stores_nothing_more() {
    let script = Script::answering(vec![ask(), review("ada"), SignInStep::Done(signed("ada"))]);
    let (service, kept) = service(&script, vec![typist(), typist(), typist()]);
    let caller = app("org.quire.Mail");
    let AccountsReply::Added(first) = added(&service, &caller).await else {
        panic!("the first add");
    };
    let held = service.registry().accounts.clone();
    let key = SecretKey {
        account: first,
        purpose: SecretPurpose::IncomingPassword,
    };
    let secret = kept.secrets.get(&key).await;

    script.steps.lock().expect("steps").extend([
        ask(),
        review("ada"),
        SignInStep::Done(signed("ada")),
    ]);
    kept.shown.lock().expect("shown").clear();
    assert_eq!(
        added(&service, &caller).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    // The sheet got as far as the review and said it was already there: no confirmation was
    // sent, and the account that was there is as it was.
    assert_eq!(
        kinds(&kept.shown),
        ["providers", "working", "form", "working", "failed"]
    );
    assert_eq!(script.told().last(), Some(&"cancel"));
    assert_eq!(service.registry().accounts, held);
    assert_eq!(kept.audit.entries().len(), 1);
    assert_eq!(kept.secrets.get(&key).await, secret);

    // Another login is another account.
    script.steps.lock().expect("steps").extend([
        ask(),
        review("bob"),
        SignInStep::Done(signed("bob")),
    ]);
    assert!(matches!(
        added(&service, &caller).await,
        AccountsReply::Added(_)
    ));
    assert_eq!(service.registry().accounts.len(), 2);
}

#[tokio::test]
async fn a_provider_that_will_not_start_a_sign_in_fails_the_sheet() {
    let script = Script::default();
    let mut refusing = provider(&script);
    refusing.refuses_to_start = true;
    let (service, _kept) = service_over(
        vec![refusing],
        Registry::default(),
        Shared::default(),
        vec![typist()],
    );
    let reply = service.handle(&app("org.quire.Mail"), add_request()).await;
    // Forbidden is not retryable; the person closes the failed sheet.
    assert_eq!(reply, AccountsReply::Refused(Refusal::Denied));
}

#[tokio::test]
async fn closing_the_sheet_cancels_the_sign_in_and_stores_nothing() {
    let script = Script::answering(vec![ask(), review("ada")]);
    let closes_at_review: Reactor = Arc::new(|view| match view {
        SheetView::Review(_) => Some(SheetInput::Dismiss),
        other => typist()(other),
    });
    let (service, kept) = service(&script, vec![closes_at_review]);
    assert_eq!(
        added(&service, &app("org.quire.Mail")).await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
    assert_eq!(script.told(), ["start", "fields", "cancel"]);
    assert!(service.registry().accounts.is_empty());
    assert_eq!(kept.store.saves(), 0);
}

#[tokio::test]
async fn a_host_with_no_sheet_is_unavailable() {
    let script = Script::default();
    let (service, _kept) = service(&script, vec![]);
    assert_eq!(
        added(&service, &app("org.quire.Mail")).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
}

#[tokio::test]
async fn a_browser_step_is_polled_until_the_sign_in_is_done() {
    let page =
        WebUrl::parse("https://login.example.org/authorize?state=1&scope=a%20b").expect("url");
    let script = Script::answering(vec![
        SignInStep::OpenBrowser { url: page.clone() },
        SignInStep::Waiting,
        SignInStep::Waiting,
        review("ada"),
        SignInStep::Done(signed("ada")),
    ]);
    let (service, kept) = service(&script, vec![typist()]);
    let AccountsReply::Added(_) = added(&service, &app("org.quire.Mail")).await else {
        panic!("added");
    };
    assert_eq!(
        script.told(),
        ["start", "poll", "poll", "poll", "confirm"],
        "the host polled on its own while the page was open"
    );
    assert_eq!(
        kinds(&kept.shown),
        [
            "providers",
            "working",
            "browser",
            "review",
            "working",
            "done"
        ]
    );
}

#[tokio::test]
async fn a_code_step_is_polled_too() {
    let url = EndpointUrl::parse("https://login.example.org/device").expect("url");
    let script = Script::answering(vec![
        SignInStep::ShowCode {
            user_code: UserCode("ABCD-EFGH".into()),
            url,
        },
        review("ada"),
        SignInStep::Done(signed("ada")),
    ]);
    let (service, kept) = service(&script, vec![typist()]);
    let AccountsReply::Added(_) = added(&service, &app("org.quire.Mail")).await else {
        panic!("added");
    };
    assert_eq!(script.told(), ["start", "poll", "confirm"]);
    assert!(kinds(&kept.shown).contains(&"code"));
}

#[tokio::test]
async fn closing_the_sheet_ends_a_wait_for_the_browser_at_once() {
    let page =
        WebUrl::parse("https://login.example.org/authorize?state=1&scope=a%20b").expect("url");
    // The sign-in would wait for ever.
    let script = Script::answering(vec![SignInStep::OpenBrowser { url: page }]);
    let gives_up: Reactor = Arc::new(|view| match view {
        SheetView::BrowserWait { .. } => Some(SheetInput::Dismiss),
        other => typist()(other),
    });
    let (service, _kept) = service(&script, vec![gives_up]);
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        added(&service, &app("org.quire.Mail")),
    )
    .await
    .expect("the dismissal is heard while the sign-in waits");
    assert_eq!(reply, AccountsReply::Refused(Refusal::Dismissed));
    assert_eq!(script.told().last(), Some(&"cancel"));
}

#[tokio::test]
async fn open_again_shows_the_browser_page_again_without_restarting_the_sign_in() {
    let page =
        WebUrl::parse("https://login.example.org/authorize?state=1&scope=a%20b").expect("url");
    let script = Script::answering(vec![SignInStep::OpenBrowser { url: page }]);
    let seen = Arc::new(Mutex::new(0u32));
    let person: Reactor = Arc::new(move |view| match view {
        SheetView::BrowserWait { .. } => {
            let mut n = seen.lock().expect("seen");
            *n += 1;
            Some(match *n {
                1 => SheetInput::OpenAgain,
                _ => SheetInput::Dismiss,
            })
        }
        other => typist()(other),
    });
    let (service, kept) = service(&script, vec![person]);
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        added(&service, &app("org.quire.Mail")),
    )
    .await
    .expect("ends");
    assert_eq!(reply, AccountsReply::Refused(Refusal::Dismissed));
    let waits = kept
        .shown
        .lock()
        .expect("shown")
        .iter()
        .filter(|v| matches!(v, SheetView::BrowserWait { .. }))
        .count();
    assert_eq!(waits, 2, "shown, then shown again for the open");
    assert_eq!(
        script.told().iter().filter(|t| **t == "start").count(),
        1,
        "the sign-in was not restarted"
    );
}

#[tokio::test]
async fn an_endpoint_porter_would_not_use_is_not_stored() {
    let mut bad = signed("ada");
    bad.endpoints = vec![ServiceEndpoint {
        family: Family::Imap,
        url: EndpointUrl::parse("imap://mail.example.org:143").expect("url"),
        tls: Tls::Plain,
        login: LoginName("ada".into()),
    }];
    let script = Script::answering(vec![ask(), review("ada"), SignInStep::Done(bad)]);
    let (service, kept) = service(&script, vec![typist()]);
    assert_eq!(
        added(&service, &app("org.quire.Mail")).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    assert!(service.registry().accounts.is_empty());
    assert_eq!(kept.audit.entries(), vec![]);
    assert!(kinds(&kept.shown).contains(&"failed"));
}

#[tokio::test]
async fn a_secret_that_cannot_be_filed_takes_back_the_ones_that_were() {
    let script = Script::answering(vec![ask(), review("ada"), SignInStep::Done(signed("ada"))]);
    let secrets = OneThenFull::default();
    let held = secrets.0.clone();
    let (service, kept) = service_over(
        vec![provider(&script)],
        Registry::default(),
        secrets,
        vec![typist()],
    );
    assert_eq!(
        service.handle(&app("org.quire.Mail"), add_request()).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    assert!(service.registry().accounts.is_empty());
    let id = AccountId::parse("scripted-ada-example.org").expect("id");
    let key = SecretKey {
        account: id,
        purpose: SecretPurpose::IncomingPassword,
    };
    assert_eq!(held.get(&key).await, Err(SecretsError::Missing));
    assert_eq!(kept.store.saves(), 0);
}

#[tokio::test]
async fn a_registry_that_cannot_be_saved_takes_back_the_account() {
    let script = Script::answering(vec![ask(), review("ada"), SignInStep::Done(signed("ada"))]);
    let (service, kept) = service(&script, vec![typist()]);
    kept.store.refusing(true);
    assert_eq!(
        added(&service, &app("org.quire.Mail")).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    assert!(service.registry().accounts.is_empty());
    let key = SecretKey {
        account: AccountId::parse("scripted-ada-example.org").expect("id"),
        purpose: SecretPurpose::OutgoingPassword,
    };
    assert_eq!(kept.secrets.get(&key).await, Err(SecretsError::Missing));
}

#[tokio::test]
async fn providers_with_nothing_to_sign_in_are_not_listed() {
    let script = Script::default();
    let runtime = ScriptedProvider {
        spec: parse_provider(LOCAL).expect("spec"),
        script: script.clone(),
        refuses_to_start: false,
    };
    let sees_list: Reactor = Arc::new(|view| match view {
        SheetView::Providers(rows) => {
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0].id.as_str(), "scripted");
            Some(SheetInput::Dismiss)
        }
        _ => None,
    });
    let (service, _kept) = service_over(
        vec![runtime, provider(&script)],
        Registry::default(),
        Shared::default(),
        vec![sees_list],
    );
    assert_eq!(
        service.handle(&app("org.quire.Mail"), add_request()).await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
}

#[tokio::test]
async fn the_providers_step_carries_the_face_a_provider_file_gives_its_mark() {
    let script = Script::default();
    let with_face = SPEC.replace(
        "mark = \"generic\"\n",
        "mark = \"generic\"\n[mark_face]\nletter = \"Sc\"\ncolour = \"#aabbcc\"\n",
    );
    let faced = ScriptedProvider {
        spec: parse_provider(&with_face).expect("spec"),
        script: script.clone(),
        refuses_to_start: false,
    };
    let sees_face: Reactor = Arc::new(|view| match view {
        SheetView::Providers(rows) => {
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0].mark, "generic");
            let face = rows[0]
                .mark_face
                .as_ref()
                .expect("the row carries the face");
            assert_eq!(
                (face.letter.as_str(), face.colour.as_str()),
                ("Sc", "#AABBCC")
            );
            Some(SheetInput::Dismiss)
        }
        _ => None,
    });
    let (service, _kept) = service_over(
        vec![faced],
        Registry::default(),
        Shared::default(),
        vec![sees_face],
    );
    assert_eq!(
        service.handle(&app("org.quire.Mail"), add_request()).await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
}

// ---- signing in again ----

fn held_account(login: &str, state: AccountState) -> Account {
    Account {
        id: AccountId::parse("scripted-ada").expect("id"),
        provider: ProviderId::parse("scripted").expect("id"),
        label: AccountLabel("ada@example.org".into()),
        state,
        auth: AuthKind::Password,
        capabilities: vec![imap_claim()],
        restriction: Restriction::none(),
        endpoints: vec![imap(login)],
    }
}

fn grant_to(caller: &AppId, account: &Account) -> Grant {
    Grant {
        id: porter_core::GrantId::parse("grant-1").expect("id"),
        key: GrantKey {
            app: caller.clone(),
            account: account.id.clone(),
            kind: CapabilityKind::Mail,
            class: DataClass::Mail,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }
}

fn reauth_service(
    script: &Script,
    state: AccountState,
    people: Vec<Reactor>,
    caller: &AppId,
) -> (Service, Kept) {
    let account = held_account("ada", state);
    let registry = Registry {
        grants: vec![grant_to(caller, &account)],
        accounts: vec![account],
        toggles: vec![],
    };
    let secrets = Shared::default();
    let (service, kept) = service_over(vec![provider(script)], registry, secrets.clone(), people);
    (service, Kept { secrets, ..kept })
}

fn reauth_request() -> AccountsRequest {
    AccountsRequest::Reauthenticate {
        account: AccountId::parse("scripted-ada").expect("id"),
        window: ParentWindow::Unparented,
    }
}

#[tokio::test]
async fn signing_in_again_replaces_the_secrets_sets_the_account_working_and_audits_it() {
    let mail = app("org.quire.Mail");
    let script = Script::answering(vec![SignInStep::Done(signed("ada"))]);
    let (service, kept) = reauth_service(&script, AccountState::NeedsReauth, vec![typist()], &mail);
    assert_eq!(
        service.handle(&mail, reauth_request()).await,
        AccountsReply::Reauthenticated
    );
    assert_eq!(
        script.modes.lock().expect("modes").clone(),
        vec![SignInMode::Reauthenticate {
            account: AccountId::parse("scripted-ada").expect("id"),
            endpoints: vec![imap("ada")],
        }]
    );
    assert_eq!(script.told(), ["start"]);
    let registry = service.registry();
    assert_eq!(registry.accounts[0].state, AccountState::Ok);
    assert_eq!(registry.accounts.len(), 1);
    let key = SecretKey {
        account: registry.accounts[0].id.clone(),
        purpose: SecretPurpose::IncomingPassword,
    };
    assert_eq!(
        kept.secrets.get(&key).await,
        Ok(Credential::Password(SecretText::new("in-pw")))
    );
    let entries = kept.audit.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, AuditEvent::Reauthed);
    assert_eq!(kept.store.saves(), 1);
    assert_eq!(kinds(&kept.shown), ["working", "done"]);
}

#[tokio::test]
async fn signing_in_again_through_the_browser_is_stored_when_the_sign_in_is_done() {
    let mail = app("org.quire.Mail");
    let page = WebUrl::parse("https://login.example.org/authorize?state=2").expect("url");
    let script = Script::answering(vec![
        SignInStep::OpenBrowser { url: page },
        SignInStep::Waiting,
        SignInStep::Done(signed("ada")),
    ]);
    let (service, kept) = reauth_service(&script, AccountState::NeedsReauth, vec![typist()], &mail);
    assert_eq!(
        service.handle(&mail, reauth_request()).await,
        AccountsReply::Reauthenticated
    );
    assert_eq!(script.told(), ["start", "poll", "poll"]);
    assert_eq!(service.registry().accounts[0].state, AccountState::Ok);
    assert_eq!(kinds(&kept.shown), ["working", "browser", "done"]);
}

#[tokio::test]
async fn a_sign_in_again_that_offers_a_review_is_never_asked_again_and_keeps_its_services() {
    let mail = app("org.quire.Mail");
    // The sign-in offers a review (as Microsoft's did), then is done once it is confirmed.
    let script = Script::answering(vec![review("ada"), SignInStep::Done(signed("ada"))]);
    let (service, kept) = reauth_service(&script, AccountState::NeedsReauth, vec![typist()], &mail);
    let before = service.registry().accounts[0].clone();
    assert_eq!(
        service.handle(&mail, reauth_request()).await,
        AccountsReply::Reauthenticated
    );
    assert_eq!(script.told(), ["start", "confirm"]);
    assert_eq!(kinds(&kept.shown), ["working", "done"]);
    let after = service.registry().accounts[0].clone();
    assert_eq!(after.state, AccountState::Ok);
    assert_eq!(after.capabilities, before.capabilities);
    assert_eq!(after.endpoints, before.endpoints);
}

#[tokio::test]
async fn another_persons_login_does_not_replace_the_credential() {
    let mail = app("org.quire.Mail");
    let script = Script::answering(vec![SignInStep::Done(signed("eve"))]);
    let (service, kept) = reauth_service(&script, AccountState::NeedsReauth, vec![typist()], &mail);
    assert_eq!(
        service.handle(&mail, reauth_request()).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    assert_eq!(
        service.registry().accounts[0].state,
        AccountState::NeedsReauth
    );
    let key = SecretKey {
        account: AccountId::parse("scripted-ada").expect("id"),
        purpose: SecretPurpose::IncomingPassword,
    };
    assert_eq!(kept.secrets.get(&key).await, Err(SecretsError::Missing));
    assert_eq!(kept.audit.entries(), vec![]);
}

#[tokio::test]
async fn an_account_whose_provider_is_gone_cannot_sign_in_again() {
    let mail = app("org.quire.Mail");
    let account = held_account("ada", AccountState::NeedsReauth);
    let registry = Registry {
        grants: vec![grant_to(&mail, &account)],
        accounts: vec![account],
        toggles: vec![],
    };
    let (service, _kept) = service_over(vec![], registry, Shared::default(), vec![typist()]);
    assert_eq!(
        service.handle(&mail, reauth_request()).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
}

#[tokio::test]
async fn a_denied_grant_is_not_a_grant_to_sign_in_again() {
    let mail = app("org.quire.Mail");
    let account = held_account("ada", AccountState::NeedsReauth);
    let mut denied = grant_to(&mail, &account);
    denied.decision = Decision::Deny;
    let registry = Registry {
        grants: vec![denied],
        accounts: vec![account],
        toggles: vec![],
    };
    let script = Script::default();
    let (service, _kept) = service_over(
        vec![provider(&script)],
        registry,
        Shared::default(),
        vec![typist()],
    );
    assert_eq!(
        service.handle(&mail, reauth_request()).await,
        AccountsReply::Refused(Refusal::UnknownGrant)
    );
}

#[tokio::test]
async fn removing_an_account_asks_the_provider_to_revoke_it_with_the_stored_account_first() {
    let mail = app("org.quire.Mail");
    let script = Script::default();
    let (service, kept) = reauth_service(&script, AccountState::Ok, vec![], &mail);
    let key = SecretKey {
        account: AccountId::parse("scripted-ada").expect("id"),
        purpose: SecretPurpose::Password,
    };
    kept.secrets
        .put(&key, &Credential::Password(SecretText::new("in-pw")))
        .await
        .expect("put");
    service.remove_account(&key.account).await.expect("removed");
    let revoked = script.revoked.lock().expect("revoked").clone();
    assert_eq!(revoked, vec![held_account("ada", AccountState::Ok)]);
    assert!(service.registry().accounts.is_empty());
    assert_eq!(kept.secrets.get(&key).await, Err(SecretsError::Missing));
}

#[tokio::test]
async fn a_provider_that_cannot_revoke_never_stops_the_removal() {
    let mail = app("org.quire.Mail");
    let script = Script::default();
    *script.revoke_fails.lock().expect("flag") = true;
    let (service, kept) = reauth_service(&script, AccountState::Ok, vec![], &mail);
    let key = SecretKey {
        account: AccountId::parse("scripted-ada").expect("id"),
        purpose: SecretPurpose::Password,
    };
    kept.secrets
        .put(&key, &Credential::Password(SecretText::new("in-pw")))
        .await
        .expect("put");
    let report = service
        .remove_with_revoke(&key.account)
        .await
        .expect("removed");
    assert_eq!(report, porter_service::RevokeReport::Skipped);
    assert_eq!(script.revoked.lock().expect("revoked").len(), 1);
    assert!(service.registry().accounts.is_empty());
    assert_eq!(kept.secrets.get(&key).await, Err(SecretsError::Missing));
    // No credential filed: the provider is not asked, and the removal goes through.
    let (service, _kept) = reauth_service(&script, AccountState::Ok, vec![], &mail);
    let before = script.revoked.lock().expect("revoked").len();
    service.remove_account(&key.account).await.expect("removed");
    assert_eq!(script.revoked.lock().expect("revoked").len(), before);
}
