//! The permits to use the person's AI keys, over the real service with in-memory secrets, a
//! scripted sheet and an in-memory registry: what an app's permits say, the key read once (and
//! only for a permit that still opens it), the audit line that says so without the key, and the
//! permit an app already holds being answered without a second sheet.

use porter_capability::{CapabilityKind, Toggle};
use porter_core::consent::{GrantScope, Usage, Verdict};
use porter_core::{AccountId, GrantId};
use porter_core::{
    AccountState, AppId, AppName, Credential, DataClass, Isolation, SecretKey, SecretPurpose,
    SecretText,
};
use porter_fake::{
    AskLog, MemoryStore, NOW, RecordingAudit, Scripted, ScriptedSheets, llm_account, llm_provider,
};
use porter_ids::clock::FixedClock;
use porter_match::llm_need;
use porter_secrets::{MemorySecrets, Secrets, SecretsError};
use porter_service::AccountService;
use porter_service_core::audit::AuditEvent;
use porter_service_core::registry::Registry;
use porter_wire::ParentWindow;
use porter_wire::{AccountsReply, AccountsRequest, Refusal};
use std::sync::Arc;

const KEY: &str = "sk-or-v1-S3CRET-CLOUD-KEY-0123456789";

/// The secret store, shared between the service that files a key and the desk that reads it.
#[derive(Debug, Clone, Default)]
struct Shared(Arc<MemorySecrets>);

impl Secrets for Shared {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        self.0.put(key, value).await
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<bool, SecretsError> {
        self.0.put_if_absent(key, value).await
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

type Service = AccountService<
    porter_fake::FakeProvider,
    Shared,
    ScriptedSheets,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

struct Rig {
    service: Arc<Service>,
    secrets: Shared,
    audit: RecordingAudit,
    asks: AskLog,
}

fn llm_key() -> SecretKey {
    SecretKey {
        account: llm_account().id,
        purpose: SecretPurpose::ApiKey,
    }
}

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

/// A rig whose language-model account holds `KEY`, with `consents` allowing answers scripted for
/// the consent sheet.
async fn rig(consents: usize) -> Rig {
    rig_scoped(GrantScope::Always, consents).await
}

/// A rig whose scripted answers all allow with `scope`.
async fn rig_scoped(scope: GrantScope, consents: usize) -> Rig {
    let secrets = Shared::default();
    secrets
        .put(&llm_key(), &Credential::ApiKey(SecretText::new(KEY)))
        .await
        .expect("key filed");
    let audit = RecordingAudit::default();
    let registry = Registry {
        accounts: vec![llm_account()],
        grants: vec![],
        toggles: vec![],
    };
    let sheets =
        ScriptedSheets::answering((0..consents).map(|_| Scripted::AllowFirst(scope.clone())));
    let asks = sheets.log();
    let service = Arc::new(
        AccountService::new(
            vec![llm_provider()],
            registry,
            secrets.clone(),
            sheets,
            FixedClock(NOW),
        )
        .with_store(MemoryStore::default())
        .with_audit(audit.clone()),
    );
    Rig {
        service,
        secrets,
        audit,
        asks,
    }
}

/// The permit the person gives `who` for prompts, as the consent sheet would.
async fn permit(rig: &Rig, who: &AppId) -> GrantId {
    let reply = rig
        .service
        .handle(
            who,
            AccountsRequest::Choose {
                need: llm_need(),
                class: DataClass::Prompt,
                usage: Usage::Interactive,
                window: ParentWindow::Unparented,
            },
        )
        .await;
    let AccountsReply::Chosen(chosen) = reply else {
        panic!("a permit, got {reply:?}");
    };
    chosen.grant
}

#[tokio::test]
async fn an_app_sees_its_permit_and_every_other_app_or_class_is_asked() {
    let rig = rig(1).await;
    let companion = app("org.quire.Companion");
    let grant = permit(&rig, &companion).await;
    let verdict = |app: AppId, class| {
        let rows = rig
            .service
            .llm_verdicts(&app, class, Usage::Interactive, &|_| None);
        assert_eq!(rows.len(), 1);
        rows[0].verdict.clone()
    };
    assert_eq!(
        verdict(companion.clone(), DataClass::Prompt),
        Verdict::Granted {
            grant: grant.clone(),
            scope: GrantScope::Always
        }
    );
    assert_eq!(verdict(companion.clone(), DataClass::Mail), Verdict::Ask);
    assert_eq!(
        verdict(app("org.quire.Other"), DataClass::Prompt),
        Verdict::Ask
    );

    // Asking again gives the same permit and draws no second sheet.
    let chosen = rig
        .service
        .existing_permit(
            &companion,
            &llm_need(),
            DataClass::Prompt,
            Usage::Interactive,
        )
        .expect("the permit it holds");
    assert_eq!(chosen.grant, grant);
    assert_eq!(rig.asks.asked().len(), 1);
}

#[tokio::test]
async fn a_permit_opens_the_key_in_process_and_the_release_is_audited_without_the_key() {
    let rig = rig(1).await;
    let companion = app("org.quire.Companion");
    let grant = permit(&rig, &companion).await;
    let key = rig.service.release_key(&grant).await.expect("key");
    assert_eq!(key.expose(), KEY);
    assert!(!format!("{key:?}").contains("S3CRET"));
    let released: Vec<_> = rig
        .audit
        .entries()
        .into_iter()
        .filter(|e| matches!(e.event, AuditEvent::KeyResolved { .. }))
        .collect();
    assert_eq!(released.len(), 1, "{:?}", rig.audit.entries());
    assert_eq!(released[0].app.as_ref(), Some(&companion));
    assert_eq!(released[0].account, Some(llm_account().id));
    let everything = serde_json::to_string(&rig.audit.entries()).expect("json");
    assert!(!everything.contains(KEY), "{everything}");
}

#[tokio::test]
async fn a_permit_that_does_not_open_a_key_is_refused() {
    let rig = rig(1).await;
    let grant = permit(&rig, &app("org.quire.Companion")).await;
    let account = llm_account().id;

    // A permit nobody holds.
    let nobody = GrantId::parse("grant-404").expect("id");
    assert_eq!(
        rig.service.release_key(&nobody).await.map(drop),
        Err(Refusal::UnknownGrant)
    );

    // The service turned off for the account, and on again.
    rig.service
        .set_toggle(&account, CapabilityKind::Llm, Toggle::Off)
        .await
        .expect("off");
    assert_eq!(
        rig.service.release_key(&grant).await.map(drop),
        Err(Refusal::Denied)
    );
    rig.service
        .set_toggle(&account, CapabilityKind::Llm, Toggle::On)
        .await
        .expect("on");
    rig.service.release_key(&grant).await.expect("on again");

    // An account that must be signed in again.
    assert!(
        rig.service
            .set_state(&account, AccountState::NeedsReauth)
            .await
    );
    assert_eq!(
        rig.service.release_key(&grant).await.map(drop),
        Err(Refusal::NeedsReauth)
    );
    assert!(rig.service.set_state(&account, AccountState::Ok).await);

    // A key that is no longer filed.
    rig.secrets.delete(&llm_key()).await.expect("delete");
    assert_eq!(
        rig.service.release_key(&grant).await.map(drop),
        Err(Refusal::NeedsReauth)
    );
    rig.secrets
        .put(&llm_key(), &Credential::ApiKey(SecretText::new(KEY)))
        .await
        .expect("key filed");

    // A revoked permit opens nothing.
    rig.service.revoke_grant(&grant).await.expect("revoke");
    assert_eq!(
        rig.service.release_key(&grant).await.map(drop),
        Err(Refusal::UnknownGrant)
    );

    // An allow-once permit is spent by the first key read that succeeds, not by one that fails.
    let once = rig_scoped(GrantScope::Once, 1).await;
    let grant = permit(&once, &app("org.quire.Companion")).await;
    once.secrets.delete(&llm_key()).await.expect("delete");
    assert_eq!(
        once.service.release_key(&grant).await.map(drop),
        Err(Refusal::NeedsReauth)
    );
    once.secrets
        .put(&llm_key(), &Credential::ApiKey(SecretText::new(KEY)))
        .await
        .expect("key filed");
    let key = once.service.release_key(&grant).await.expect("first read");
    assert_eq!(key.expose(), KEY);
    assert_eq!(
        once.service.release_key(&grant).await.map(drop),
        Err(Refusal::UnknownGrant)
    );
}

#[tokio::test]
async fn an_account_that_is_not_an_own_server_has_no_server_key_and_lists_no_server() {
    let rig = rig(0).await;
    assert!(rig.service.own_servers().is_empty());
    assert_eq!(
        rig.service
            .own_server_key(&llm_account().id)
            .await
            .map(drop),
        Err(Refusal::UnknownGrant)
    );
}
