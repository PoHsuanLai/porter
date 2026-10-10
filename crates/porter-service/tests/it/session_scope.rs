//! The service half of "This session only" (lane session-scope): the sheet is told of the session
//! the request was made in, a session scope is recorded only for that session, and ending a
//! session removes exactly its grants.

use porter_core::audit::AuditEvent;
use porter_core::capability::LlmFeature;
use porter_core::consent::{Decision, GrantScope, Usage};
use porter_core::need::LlmNeed;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountsReply, AccountsRequest, AppId, AppName, DataClass, Isolation, LauncherSession, Need,
    Tokens,
};
use porter_fake::{
    AskLog, FixedClock, MemoryStore, RecordingAudit, Scripted, ScriptedSheets, cloud_provider,
    llm_account, llm_provider,
};
use porter_secrets::MemorySecrets;
use porter_service::{AccountService, Registry};
use std::collections::BTreeSet;

type Svc = AccountService<
    porter_fake::FakeProvider,
    MemorySecrets,
    ScriptedSheets,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

fn session(id: &str) -> LauncherSession {
    LauncherSession::parse(id).expect("session")
}

fn agent() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Agent.claude-code").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn llm() -> Need {
    Need::Llm(LlmNeed::new(BTreeSet::from([LlmFeature::Chat]), Tokens(0)))
}

fn service(script: impl IntoIterator<Item = Scripted>) -> (Svc, AskLog, RecordingAudit) {
    let sheets = ScriptedSheets::answering(script);
    let log = sheets.log();
    let audit = RecordingAudit::default();
    let service = AccountService::new(
        vec![llm_provider(), cloud_provider()],
        Registry {
            accounts: vec![llm_account()],
            ..Registry::default()
        },
        MemorySecrets::default(),
        sheets,
        FixedClock(porter_fake::NOW),
    )
    .with_store(MemoryStore::default())
    .with_audit(audit.clone());
    (service, log, audit)
}

async fn ask_for_agent(service: &Svc, open: Option<&LauncherSession>) -> AccountsReply {
    service
        .choose_for_agent(
            &agent(),
            llm(),
            (DataClass::Prompt, Usage::Interactive),
            &ParentWindow::Unparented,
            open,
        )
        .await
}

fn scopes(service: &Svc) -> Vec<GrantScope> {
    service
        .registry()
        .grants
        .iter()
        .map(|g| g.scope.clone())
        .collect()
}

#[tokio::test]
async fn the_ask_carries_the_session_only_when_the_request_was_made_in_one() {
    let sess = session("sess-1");
    let (service, log, _) = service([Scripted::Dismiss, Scripted::Dismiss, Scripted::Dismiss]);
    ask_for_agent(&service, Some(&sess)).await;
    ask_for_agent(&service, None).await;
    // An app's own Choose is never in a session.
    service
        .handle(
            &agent(),
            AccountsRequest::Choose {
                need: llm(),
                class: DataClass::Prompt,
                usage: Usage::Interactive,
                window: ParentWindow::Unparented,
            },
        )
        .await;
    let asked = log.asked();
    assert_eq!(
        asked.iter().map(|a| a.session.clone()).collect::<Vec<_>>(),
        [Some(sess), None, None]
    );
    assert!(asked.iter().all(|a| a.app == agent()));
}

#[tokio::test]
async fn a_session_answer_is_recorded_for_the_session_the_ask_carried() {
    let sess = session("sess-1");
    let (service, _, audit) = service([Scripted::AllowFirst(GrantScope::Session(sess.clone()))]);
    let AccountsReply::Chosen(chosen) = ask_for_agent(&service, Some(&sess)).await else {
        panic!("a grant");
    };
    let registry = service.registry();
    let [grant] = registry.grants.as_slice() else {
        panic!("one grant");
    };
    assert_eq!(grant.id, chosen.grant);
    assert_eq!(grant.scope, GrantScope::Session(sess));
    assert_eq!(grant.decision, Decision::Allow);
    assert_eq!(grant.key.app, agent());
    assert!(
        audit
            .entries()
            .iter()
            .any(|e| matches!(e.event, AuditEvent::Granted { .. }) && e.app == Some(agent()))
    );
}

#[tokio::test]
async fn a_session_scope_the_ask_did_not_offer_makes_no_grant() {
    let (service, _, _) = service([
        // Another session than the one asked in.
        Scripted::AllowFirst(GrantScope::Session(session("sess-2"))),
        // A session scope on an ask that had none.
        Scripted::AllowFirst(GrantScope::Session(session("sess-1"))),
        // And on an app's own Choose.
        Scripted::AllowFirst(GrantScope::Session(session("sess-1"))),
    ]);
    let sess = session("sess-1");
    assert_eq!(
        ask_for_agent(&service, Some(&sess)).await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
    assert_eq!(
        ask_for_agent(&service, None).await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
    assert_eq!(
        service
            .handle(
                &agent(),
                AccountsRequest::Choose {
                    need: llm(),
                    class: DataClass::Prompt,
                    usage: Usage::Interactive,
                    window: ParentWindow::Unparented,
                },
            )
            .await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
    assert!(scopes(&service).is_empty());
}

#[tokio::test]
async fn once_and_always_still_work_in_a_session_ask() {
    let sess = session("sess-1");
    let (service, _, _) = service([
        Scripted::AllowFirst(GrantScope::Always),
        Scripted::AllowFirst(GrantScope::Once),
    ]);
    ask_for_agent(&service, Some(&sess)).await;
    // The second ask is for the same key; the sheet is asked again (the service does not
    // consult the store for a Choose), and the newer grant is the one that decides.
    ask_for_agent(&service, Some(&sess)).await;
    assert_eq!(scopes(&service), [GrantScope::Always, GrantScope::Once]);
}

#[tokio::test]
async fn ending_a_session_removes_its_grants_alone_and_audits_each() {
    let (one, two) = (session("sess-1"), session("sess-2"));
    let (service, _, audit) = service([
        Scripted::AllowFirst(GrantScope::Session(one.clone())),
        Scripted::AllowFirst(GrantScope::Always),
        Scripted::AllowFirst(GrantScope::Session(two.clone())),
    ]);
    ask_for_agent(&service, Some(&one)).await;
    ask_for_agent(&service, Some(&one)).await;
    ask_for_agent(&service, Some(&two)).await;
    let before = service.registry().grants;
    assert_eq!(before.len(), 3);

    let ended = service.end_session_grants(Some(&one)).await;
    assert_eq!(ended, [before[0].id.clone()]);
    assert_eq!(
        scopes(&service),
        [GrantScope::Always, GrantScope::Session(two.clone())]
    );
    let audited: Vec<_> = audit
        .entries()
        .into_iter()
        .filter_map(|e| match e.event {
            AuditEvent::SessionGrantEnded { grant, session } => Some((grant, session, e.app)),
            _ => None,
        })
        .collect();
    assert_eq!(audited, [(before[0].id.clone(), one, Some(agent()))]);

    // Ending a session nobody holds ends nothing; no session names every one of them.
    assert!(
        service
            .end_session_grants(Some(&session("sess-9")))
            .await
            .is_empty()
    );
    assert_eq!(service.end_session_grants(None).await.len(), 1);
    assert_eq!(scopes(&service), [GrantScope::Always]);
}

#[tokio::test]
async fn add_account_in_a_session_ask_is_dismissed_not_granted_for_good() {
    let sess = session("sess-1");
    let (service, _, _) = service([Scripted::AddAccount]);
    assert_eq!(
        ask_for_agent(&service, Some(&sess)).await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
    assert!(scopes(&service).is_empty());
}
