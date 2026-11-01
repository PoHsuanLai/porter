//! Changing the registry when it cannot be saved: what is left in memory, in the audit log and in
//! the store.
//!
//! Every change goes through `Ctx::transact`: a registry that cannot be saved is left as it was,
//! with no audit line, and the person is told `Unavailable`. (These began as pins of the code
//! before it, whose behaviour is in the history of this file.)

use porter_capability::{CapabilityKind, Toggle};
use porter_core::consent::{ConsentAnswer, ConsentAsk, GrantScope, Usage};
use porter_core::{AccountState, AgentState, AppId, AppName, DataClass, Isolation, SessionToken};
use porter_core::{AuthKind, GrantId};
use porter_fake::{
    FakeProvider, MemoryStore, RecordingAudit, Scripted, ScriptedLink, ScriptedSheets,
    cloud_provider, fake_service, llm_account, mail_account, mail_provider, storage_account,
};
use porter_ids::clock::FixedClock;
use porter_match::{Need, StorageNeed};
use porter_secrets::{MemorySecrets, SecretsError};
use porter_service::AccountService;
use porter_service_accounts::AgentFault;
use porter_service_core::audit::AuditEvent;
use porter_service_core::registry::Registry;
use porter_sheet::host::{SheetFault, SheetOpen, Sheets};
use porter_wire::{AccountsReply, ParentWindow, Refusal};
use std::sync::Arc;
use tokio::sync::Notify;

type Svc = AccountService<
    FakeProvider,
    MemorySecrets,
    ScriptedSheets,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

struct Rig {
    service: Svc,
    store: MemoryStore,
    audit: RecordingAudit,
}

async fn rig(script: impl IntoIterator<Item = Scripted>) -> Rig {
    let store = MemoryStore::default();
    let audit = RecordingAudit::default();
    let service = fake_service(ScriptedSheets::answering(script))
        .await
        .with_store(store.clone())
        .with_audit(audit.clone());
    Rig {
        service,
        store,
        audit,
    }
}

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Photos").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn files() -> Need {
    use porter_capability::{Access, Delta, StorageScope};
    Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        false,
    ))
}

fn session(id: &str) -> SessionToken {
    SessionToken::parse(id).expect("session")
}

impl Rig {
    /// Asks for the storage account's files, answered `scope`, while the store works.
    async fn grant(&self, scope: GrantScope, session: Option<&SessionToken>) -> GrantId {
        let asked = self
            .service
            .choose(
                &app(),
                files(),
                (DataClass::Files, Usage::Interactive),
                None,
                &ParentWindow::Unparented,
                session,
            )
            .await;
        let AccountsReply::Chosen(candidate) = asked else {
            panic!("not chosen: {asked:?} ({scope:?})");
        };
        candidate.grant
    }

    /// From here on nothing can be saved, and what the registry, the audit log and the store
    /// hold now is what a refused change is measured against.
    fn stop_saving(&self) -> Before {
        self.store.refusing(true);
        Before {
            registry: self.service.registry(),
            audit: self.audit.entries().len(),
            saves: self.store.saves(),
        }
    }
}

struct Before {
    registry: Registry,
    audit: usize,
    saves: usize,
}

impl Before {
    fn registry_changed(&self, rig: &Rig) -> bool {
        rig.service.registry() != self.registry
    }

    fn audit_lines(&self, rig: &Rig) -> Vec<AuditEvent> {
        rig.audit
            .entries()
            .into_iter()
            .skip(self.audit)
            .map(|e| e.event)
            .collect()
    }

    fn store_untouched(&self, rig: &Rig) {
        assert_eq!(rig.store.saves(), self.saves, "nothing was saved");
    }
}

#[tokio::test]
async fn a_grant_that_cannot_be_saved() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Always)]).await;
    let before = rig.stop_saving();
    let asked = rig
        .service
        .choose(
            &app(),
            files(),
            (DataClass::Files, Usage::Interactive),
            None,
            &ParentWindow::Unparented,
            None,
        )
        .await;
    assert_eq!(asked, AccountsReply::Refused(Refusal::Unavailable));
    before.store_untouched(&rig);
    assert!(!before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn a_revoke_that_cannot_be_saved() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Always)]).await;
    let grant = rig.grant(GrantScope::Always, None).await;
    let before = rig.stop_saving();
    assert_eq!(
        rig.service.revoke(&app(), &grant).await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    assert!(!before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn a_revoke_from_settings_that_cannot_be_saved() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Always)]).await;
    let grant = rig.grant(GrantScope::Always, None).await;
    let before = rig.stop_saving();
    assert_eq!(
        rig.service.revoke_grant(&grant).await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    assert!(!before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn the_spending_of_a_once_grant_that_cannot_be_saved() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Once)]).await;
    let grant = rig.grant(GrantScope::Once, None).await;
    let before = rig.stop_saving();
    assert_eq!(
        rig.service.spend_once(&app(), &grant).await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    assert!(!before.registry_changed(&rig), "still held");
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn the_end_of_a_session_that_cannot_be_saved() {
    let sess = session("sess-1");
    let rig = rig([Scripted::AllowFirst(GrantScope::Session(sess.clone()))]).await;
    let grant = rig
        .grant(GrantScope::Session(sess.clone()), Some(&sess))
        .await;
    let before = rig.stop_saving();
    let ended = rig.service.end_session_grants(Some(&sess)).await;
    before.store_untouched(&rig);
    // Nothing ended: none is reported, and nothing is logged.
    assert_eq!(ended, vec![]);
    assert!(!before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
    assert!(rig.service.registry().grants.iter().any(|g| g.id == grant));
}

#[tokio::test]
async fn a_toggle_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.stop_saving();
    assert_eq!(
        rig.service
            .set_toggle(&storage_account().id, CapabilityKind::Calendar, Toggle::Off)
            .await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    assert!(!before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn a_toggle_that_changes_nothing_still_saves() {
    let rig = rig([]).await;
    let id = storage_account().id;
    rig.service
        .set_toggle(&id, CapabilityKind::Calendar, Toggle::On)
        .await
        .expect("toggled");
    assert_eq!(rig.store.saves(), 0, "the kind was already on");
    assert_eq!(rig.audit.entries(), vec![]);
}

#[tokio::test]
async fn a_removal_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.stop_saving();
    let removed = rig.service.remove_account(&storage_account().id).await;
    before.store_untouched(&rig);
    assert_eq!(removed, Err(SecretsError::Unavailable));
    assert!(!before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn a_look_at_an_account_again_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.stop_saving();
    let looked = rig.service.rediscover(&storage_account().id).await;
    assert_eq!(looked.err(), Some(Refusal::Unavailable));
    before.store_untouched(&rig);
    assert!(!before.registry_changed(&rig));
}

#[tokio::test]
async fn a_state_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.stop_saving();
    let changed = rig
        .service
        .set_state(&mail_account().id, AccountState::NeedsReauth)
        .await;
    before.store_untouched(&rig);
    assert!(!changed);
    assert!(!before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn an_agent_state_that_cannot_be_saved() {
    let store = MemoryStore::default();
    let audit = RecordingAudit::default();
    let agent = {
        let mut account = llm_account();
        account.auth = AuthKind::AgentLogin;
        account
    };
    let id = agent.id.clone();
    let registry = Registry {
        accounts: vec![agent],
        ..Registry::default()
    };
    let service = AccountService::new(
        vec![cloud_provider(), mail_provider()],
        registry,
        MemorySecrets::default(),
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    )
    .with_store(store.clone())
    .with_audit(audit.clone());
    store.refusing(true);
    let before = service.registry();
    assert_eq!(
        service.set_agent_state(&id, AgentState::NeedsLogin).await,
        Err(AgentFault::Unavailable)
    );
    assert_eq!(service.registry(), before);
    assert_eq!(audit.entries(), vec![]);
    assert_eq!(store.saves(), 0);
}

/// A consent sheet that waits for the test before it answers "Allow" for the first account.
struct Held {
    asked: Arc<Notify>,
    answer: Arc<Notify>,
}

impl Sheets for Held {
    type Link = ScriptedLink;

    async fn conversation(&self, _open: SheetOpen) -> Result<ScriptedLink, SheetFault> {
        Err(SheetFault::Unavailable)
    }

    async fn consent(&self, ask: ConsentAsk, _window: &ParentWindow) -> ConsentAnswer {
        let account = ask.accounts.first().expect("an account").account.clone();
        self.asked.notify_one();
        self.answer.notified().await;
        ConsentAnswer::Allow {
            account,
            scope: GrantScope::Always,
        }
    }
}

#[tokio::test]
async fn an_account_removed_while_its_consent_sheet_is_open_gets_no_grant() {
    let (asked, answer) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let store = MemoryStore::default();
    let audit = RecordingAudit::default();
    let registry = Registry {
        accounts: vec![storage_account()],
        ..Registry::default()
    };
    let service = AccountService::new(
        vec![cloud_provider()],
        registry,
        MemorySecrets::default(),
        Held {
            asked: asked.clone(),
            answer: answer.clone(),
        },
        FixedClock(porter_fake::NOW),
    )
    .with_store(store.clone())
    .with_audit(audit.clone());
    let (me, window) = (app(), ParentWindow::Unparented);
    let choosing = service.choose(
        &me,
        files(),
        (DataClass::Files, Usage::Interactive),
        None,
        &window,
        None,
    );
    let removing = async {
        asked.notified().await;
        service
            .remove_account(&storage_account().id)
            .await
            .expect("removed");
        answer.notify_one();
    };
    let (reply, ()) = tokio::join!(choosing, removing);
    assert_eq!(reply, AccountsReply::Refused(Refusal::Dismissed));
    assert!(service.registry().grants.is_empty());
    assert!(service.registry().accounts.is_empty());
    let granted =
        |e: &porter_service_core::audit::AuditEntry| matches!(e.event, AuditEvent::Granted { .. });
    assert!(!audit.entries().iter().any(granted));
}

// ---- `Ctx::transact` itself ----

fn toggle_row(kind: CapabilityKind) -> porter_core::store::AccountToggle {
    porter_core::store::AccountToggle {
        account: storage_account().id,
        kind,
        toggle: Toggle::Off,
    }
}

fn ctx(
    store: &MemoryStore,
    audit: &RecordingAudit,
) -> porter_service_core::Ctx<
    porter_service_core::Parts<
        FakeProvider,
        MemorySecrets,
        ScriptedSheets,
        FixedClock,
        MemoryStore,
        RecordingAudit,
    >,
> {
    porter_service_core::Ctx::new(
        vec![cloud_provider()],
        Registry::default(),
        MemorySecrets::default(),
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    )
    .with_store(store.clone())
    .with_audit(audit.clone())
}

fn line(event: AuditEvent) -> porter_service_core::Audited {
    porter_service_core::Audited::new(None, None, event)
}

#[tokio::test]
async fn a_change_is_saved_swapped_in_audited_and_announced_in_that_order() {
    let (store, audit) = (MemoryStore::default(), RecordingAudit::default());
    let told = Arc::new(std::sync::Mutex::new(Vec::new()));
    let ctx = {
        let (told, store, audit) = (told.clone(), store.clone(), audit.clone());
        ctx(&store, &audit).on_committed(move |committed| {
            // By the time it is told, the change is saved and audited.
            let saved = store.stored().expect("saved");
            told.lock()
                .expect("told")
                .push((committed, saved, audit.entries().len()));
        })
    };
    let written = ctx
        .transact(|registry| {
            registry.toggles.push(toggle_row(CapabilityKind::Calendar));
            Ok((vec![line(AuditEvent::Removed)], 7))
        })
        .await
        .expect("written");
    assert_eq!((written.value, written.changed), (7, true));
    let told = told.lock().expect("told");
    assert_eq!(told.len(), 1);
    let (committed, saved, audited) = &told[0];
    assert_eq!(committed.before, Registry::default());
    assert_eq!(committed.after, *ctx.lock());
    assert_eq!(*saved, committed.after.persisted());
    assert_eq!(*audited, 1);
}

#[tokio::test]
async fn a_decision_that_refuses_or_changes_nothing_saves_and_announces_nothing() {
    let (store, audit) = (MemoryStore::default(), RecordingAudit::default());
    let told = Arc::new(std::sync::Mutex::new(0));
    let ctx = {
        let told = told.clone();
        ctx(&store, &audit).on_committed(move |_| *told.lock().expect("told") += 1)
    };
    let refused = ctx
        .transact(|registry| {
            registry.toggles.push(toggle_row(CapabilityKind::Calendar));
            Err::<(Vec<_>, ()), _>(Refusal::UnknownGrant)
        })
        .await;
    assert_eq!(refused, Err(Refusal::UnknownGrant));
    assert_eq!(
        *ctx.lock(),
        Registry::default(),
        "a refusal changes nothing"
    );

    let unchanged = ctx
        .transact(|_| Ok((vec![line(AuditEvent::Removed)], ())))
        .await
        .expect("written");
    assert!(!unchanged.changed);
    assert_eq!(store.saves(), 0);
    assert_eq!(*told.lock().expect("told"), 0);
    assert_eq!(audit.entries().len(), 1, "its line is still written");
}

#[tokio::test]
async fn a_change_that_cannot_be_saved_is_neither_kept_audited_nor_announced() {
    let (store, audit) = (MemoryStore::default(), RecordingAudit::default());
    let told = Arc::new(std::sync::Mutex::new(0));
    let ctx = {
        let told = told.clone();
        ctx(&store, &audit).on_committed(move |_| *told.lock().expect("told") += 1)
    };
    store.refusing(true);
    let written = ctx
        .transact(|registry| {
            registry.toggles.push(toggle_row(CapabilityKind::Calendar));
            Ok((vec![line(AuditEvent::Removed)], ()))
        })
        .await;
    assert_eq!(written, Err(Refusal::Unavailable));
    assert_eq!(*ctx.lock(), Registry::default());
    assert_eq!(audit.entries(), vec![]);
    assert_eq!(*told.lock().expect("told"), 0);
}

/// Two transactions at once each decide on the registry the other left: no change is lost.
#[tokio::test]
async fn transactions_that_overlap_run_one_after_the_other() {
    let (store, audit) = (MemoryStore::default(), RecordingAudit::default());
    let ctx = ctx(&store, &audit);
    let add = |kind| {
        ctx.transact(move |registry| {
            registry.toggles.push(toggle_row(kind));
            Ok((Vec::new(), ()))
        })
    };
    let (first, second) = tokio::join!(add(CapabilityKind::Calendar), add(CapabilityKind::Storage));
    first.expect("first");
    second.expect("second");
    assert_eq!(ctx.lock().toggles.len(), 2);
    assert_eq!(store.stored().expect("saved").toggles.len(), 2);
}

/// What accountd's publisher relies on: the service tells of a change after the registry holds
/// it (a watcher that reads then sees the change), and tells of nothing that was not saved.
#[tokio::test]
async fn the_service_tells_of_a_change_only_once_it_is_in_the_registry() {
    let told = Arc::new(Notify::new());
    let store = MemoryStore::default();
    let service = fake_service(ScriptedSheets::default())
        .await
        .with_store(store.clone())
        .on_committed({
            let told = Arc::clone(&told);
            move |_| told.notify_one()
        });
    let wake = std::time::Duration::from_millis(50);

    store.refusing(true);
    let id = storage_account().id;
    let off = || service.set_toggle(&id, CapabilityKind::Calendar, Toggle::Off);
    assert_eq!(off().await, Err(Refusal::Unavailable));
    assert!(tokio::time::timeout(wake, told.notified()).await.is_err());

    store.refusing(false);
    let (changed, seen) = tokio::join!(off(), async {
        told.notified().await;
        service.registry().toggles.len()
    });
    changed.expect("toggled");
    assert_eq!(seen, 1, "the registry already holds the change");
}
