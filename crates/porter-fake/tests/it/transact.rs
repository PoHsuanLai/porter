//! Changing the registry when it cannot be saved: what is left in memory, in the audit log and in
//! the store.
//!
//! Written against the code before `transact` and kept as the pin of what changes. Each test
//! marks the assertions that describe the old behaviour with `OLD:` and says what they become
//! with `NEW:`; the commit that moves the mutations onto `transact` flips exactly those.

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
use porter_secrets::MemorySecrets;
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
    fn from_here_unsaveable(&self) -> Before {
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
    let before = rig.from_here_unsaveable();
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
    // OLD: the grant stays in memory and "Granted" is already in the log.
    // NEW: neither (the registry is as it was, the log has no line).
    assert!(before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig).len(), 1);
}

#[tokio::test]
async fn a_revoke_that_cannot_be_saved() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Always)]).await;
    let grant = rig.grant(GrantScope::Always, None).await;
    let before = rig.from_here_unsaveable();
    assert_eq!(
        rig.service.revoke(&app(), &grant).await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    // OLD: the grant is gone from memory and "Revoked" is logged. NEW: neither.
    assert!(before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig).len(), 1);
}

#[tokio::test]
async fn a_revoke_from_settings_that_cannot_be_saved() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Always)]).await;
    let grant = rig.grant(GrantScope::Always, None).await;
    let before = rig.from_here_unsaveable();
    assert_eq!(
        rig.service.revoke_grant(&grant).await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    // OLD: gone from memory, "Revoked" logged. NEW: neither.
    assert!(before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig).len(), 1);
}

#[tokio::test]
async fn the_spending_of_a_once_grant_that_cannot_be_saved() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Once)]).await;
    let grant = rig.grant(GrantScope::Once, None).await;
    let before = rig.from_here_unsaveable();
    assert_eq!(
        rig.service.spend_once(&app(), &grant).await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    // OLD: spent in memory although the answer is Unavailable. NEW: still held.
    assert!(before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![]);
}

#[tokio::test]
async fn the_end_of_a_session_that_cannot_be_saved() {
    let sess = session("sess-1");
    let rig = rig([Scripted::AllowFirst(GrantScope::Session(sess.clone()))]).await;
    let grant = rig
        .grant(GrantScope::Session(sess.clone()), Some(&sess))
        .await;
    let before = rig.from_here_unsaveable();
    let ended = rig.service.end_session_grants(Some(&sess)).await;
    before.store_untouched(&rig);
    // OLD: the error is dropped; the grant is gone in memory, "SessionGrantEnded" is logged and
    // the grant is reported ended.
    // NEW: nothing ended, so none is reported and nothing is logged.
    assert_eq!(ended, vec![grant]);
    assert!(before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig).len(), 1);
}

#[tokio::test]
async fn a_toggle_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.from_here_unsaveable();
    assert_eq!(
        rig.service
            .set_toggle(&storage_account().id, CapabilityKind::Calendar, Toggle::Off)
            .await,
        Err(Refusal::Unavailable)
    );
    before.store_untouched(&rig);
    // OLD: switched off in memory and "ServiceToggled" logged. NEW: neither.
    assert!(before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig).len(), 1);
}

#[tokio::test]
async fn a_toggle_that_changes_nothing_still_saves() {
    let rig = rig([]).await;
    let id = storage_account().id;
    rig.service
        .set_toggle(&id, CapabilityKind::Calendar, Toggle::On)
        .await
        .expect("toggled");
    // OLD: saved although the kind was already on. NEW: 0 saves, and no audit line (as now).
    assert_eq!(rig.store.saves(), 1);
    assert_eq!(rig.audit.entries(), vec![]);
}

#[tokio::test]
async fn a_removal_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.from_here_unsaveable();
    let removed = rig.service.remove_account(&storage_account().id).await;
    before.store_untouched(&rig);
    // OLD: the error is dropped: Ok, the account gone from memory, "Removed" logged.
    // NEW: Err(Unavailable), the account is still there, nothing is logged.
    assert_eq!(removed, Ok(()));
    assert!(before.registry_changed(&rig));
    assert_eq!(before.audit_lines(&rig), vec![AuditEvent::Removed]);
}

#[tokio::test]
async fn a_look_at_an_account_again_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.from_here_unsaveable();
    let looked = rig.service.rediscover(&storage_account().id).await;
    assert_eq!(looked.err(), Some(Refusal::Unavailable));
    before.store_untouched(&rig);
    // OLD: what was found stays in memory. NEW: the registry is as it was.
    assert!(before.registry_changed(&rig));
}

#[tokio::test]
async fn a_state_that_cannot_be_saved() {
    let rig = rig([]).await;
    let before = rig.from_here_unsaveable();
    let changed = rig
        .service
        .set_state(&mail_account().id, AccountState::NeedsReauth)
        .await;
    before.store_untouched(&rig);
    // OLD: reported changed, kept in memory, the error dropped. NEW: not changed (false).
    assert!(changed);
    assert!(before.registry_changed(&rig));
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
    // OLD: the state stays in memory. NEW: the registry is as it was.
    assert_ne!(service.registry(), before);
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
