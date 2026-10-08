//! A token an app receives (`IssueToken`) and one a relay presents (`OpenAuthenticated`) are
//! asked of the provider session for the grant's kind (`access_token_for`), so a family whose
//! tokens are per kind (Microsoft's Graph, Google's APIs) gives a calendar grant a calendar
//! token and nothing wider (sec-1).

use porter_core::consent::{Decision, GrantScope, Usage};
use porter_core::consent::{Grant, GrantKey};
use porter_core::{
    AccountsReply, AccountsRequest, AppId, AppName, Audience, CapabilityKind, Credential,
    DataClass, GrantId, Isolation, IssuedToken, RelayAuth, SecretKey, SecretPurpose, SecretText,
    SpaceScope, UnixSeconds,
};
use porter_fake::{
    FakeProvider, FixedClock, MemoryStore, RecordingAudit, ScriptedSheets, mail_account,
    mail_provider,
};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignInStart,
};
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use std::sync::{Arc, Mutex};

/// The kind each token was asked for: `None` for a token asked with no kind.
type Asked = Arc<Mutex<Vec<Option<CapabilityKind>>>>;

/// The fake provider, noting the kind of every token its sessions are asked for.
#[derive(Debug, Clone)]
struct Noting {
    inner: FakeProvider,
    asked: Asked,
}

struct NotingSession {
    inner: <FakeProvider as Provider>::Session,
    asked: Asked,
}

impl ProviderSession for NotingSession {
    async fn access_token(&self, audience: &Audience) -> Result<IssuedToken, ProviderError> {
        self.asked.lock().unwrap().push(None);
        self.inner.access_token(audience).await
    }

    async fn access_token_for(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
    ) -> Result<IssuedToken, ProviderError> {
        self.asked.lock().unwrap().push(Some(kind));
        self.inner.access_token(audience).await
    }

    fn renewed(&self) -> Option<Credential> {
        self.inner.renewed()
    }
}

impl Provider for Noting {
    type Session = NotingSession;
    type SignIn = <FakeProvider as Provider>::SignIn;

    fn spec(&self) -> &ProviderSpec {
        self.inner.spec()
    }

    async fn discover(
        &self,
        account: &porter_core::Account,
        presented: &Presented,
    ) -> Result<Vec<porter_core::Claim>, ProviderError> {
        self.inner.discover(account, presented).await
    }

    async fn open(
        &self,
        account: &porter_core::AccountId,
        presented: Presented,
    ) -> Result<NotingSession, ProviderError> {
        Ok(NotingSession {
            inner: self.inner.open(account, presented).await?,
            asked: Arc::clone(&self.asked),
        })
    }

    fn sign_in(&self, start: SignInStart) -> Result<Self::SignIn, ProviderError> {
        self.inner.sign_in(start)
    }

    async fn revoke(
        &self,
        account: &porter_core::Account,
        presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        self.inner.revoke(account, presented).await
    }
}

type Svc =
    AccountService<Noting, MemorySecrets, ScriptedSheets, FixedClock, MemoryStore, RecordingAudit>;

fn mail_app() -> AppId {
    AppId {
        name: AppName::parse("org.example.Mail").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn grant_id() -> GrantId {
    GrantId::parse("mail-grant").expect("id")
}

/// The OAuth mail account, its refresh token filed, and an allowing mail grant for the app.
async fn service() -> (Svc, Asked) {
    let account = mail_account();
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: account.id.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: SecretText::new("stale"),
                refresh: SecretText::new("refresh"),
                expires_at: UnixSeconds(0),
            },
        )
        .await
        .expect("filed");
    let grant = Grant {
        id: grant_id(),
        key: GrantKey {
            app: mail_app(),
            account: account.id.clone(),
            kind: CapabilityKind::Mail,
            class: DataClass::Mail,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    };
    let asked = Asked::default();
    let provider = Noting {
        inner: mail_provider(),
        asked: Arc::clone(&asked),
    };
    let service = AccountService::new(
        vec![provider],
        Registry {
            accounts: vec![account],
            grants: vec![grant],
            ..Registry::default()
        },
        secrets,
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    )
    .with_store(MemoryStore::default())
    .with_audit(RecordingAudit::default());
    (service, asked)
}

#[tokio::test]
async fn a_token_issued_to_an_app_is_asked_for_the_grants_kind() {
    let (service, asked) = service().await;
    let reply = service
        .handle(
            &mail_app(),
            AccountsRequest::IssueToken {
                grant: grant_id(),
                audience: Audience("imap".into()),
            },
        )
        .await;
    assert!(matches!(reply, AccountsReply::Token(_)), "{reply:?}");
    assert_eq!(*asked.lock().unwrap(), [Some(CapabilityKind::Mail)]);
}

#[tokio::test]
async fn a_token_a_relay_presents_is_asked_for_the_grants_kind() {
    let (service, asked) = service().await;
    let imap = mail_account().endpoints[0].url.clone();
    let plan = service
        .open_authenticated(&mail_app(), &grant_id(), &imap)
        .await
        .expect("plan");
    assert!(matches!(plan.auth, RelayAuth::AccessToken(_)));
    assert_eq!(plan.kind, CapabilityKind::Mail);
    assert_eq!(*asked.lock().unwrap(), [Some(CapabilityKind::Mail)]);
}
