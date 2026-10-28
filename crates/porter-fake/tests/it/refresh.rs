//! Refresh safety (split plan A3): an account refreshes one caller at a time inside a store, and
//! a refresh token the issuer rotated is stored before the new access token is handed out. The
//! issuer here is a provider that counts the calls made to it and rotates the refresh token on
//! each, as Microsoft's does. No timing is involved: the issuer yields once while the first
//! caller holds the account's turn, so the second is queued behind it at any load.

use porter_capability::Claim;
use porter_capability::{Access, Delta};
use porter_core::AccountId;
use porter_core::consent::{GrantScope, Usage};
use porter_core::{
    Account, AppId, AppName, Audience, Candidate, Credential, DataClass, Isolation, IssuedToken,
    SecretKey, SecretPurpose, SecretText, TokenKind,
};
use porter_fake::{
    FakeProvider, FakeSignIn, MemoryStore, NOW, Scripted, ScriptedSheets, mail_account,
    mail_provider,
};
use porter_ids::UnixSeconds;
use porter_ids::clock::FixedClock;
use porter_match::MailNeed;
use porter_match::Need;
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignInStart,
};
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use porter_wire::ParentWindow;
use porter_wire::{AccountsReply, AccountsRequest};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What the issuer saw: how many tokens it minted, and the refresh token it was shown each time.
#[derive(Debug, Clone, Default)]
struct Seen {
    minted: Arc<AtomicUsize>,
    refreshes: Arc<Mutex<Vec<String>>>,
    /// The issuer says the grant was never given the scope asked (`invalid_scope`, AADSTS65001).
    unconsented: Arc<Mutex<bool>>,
}

impl Seen {
    fn minted(&self) -> usize {
        self.minted.load(Ordering::SeqCst)
    }

    fn refreshes(&self) -> Vec<String> {
        self.refreshes.lock().expect("refreshes").clone()
    }
}

/// The fake mail provider with an issuer that rotates the refresh token on every refresh.
#[derive(Debug, Clone)]
struct Rotating {
    inner: FakeProvider,
    seen: Seen,
}

struct RotatingSession {
    seen: Seen,
    refresh: String,
    renewed: Mutex<Option<Credential>>,
}

impl Provider for Rotating {
    type Session = RotatingSession;
    type SignIn = FakeSignIn;

    fn spec(&self) -> &ProviderSpec {
        self.inner.spec()
    }

    async fn discover(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        self.inner.discover(account, presented).await
    }

    async fn open(
        &self,
        _account: &AccountId,
        presented: Presented,
    ) -> Result<RotatingSession, ProviderError> {
        let Presented::Credential(Credential::OAuth { refresh, .. }) = presented else {
            return Err(ProviderError::Unauthorized);
        };
        Ok(RotatingSession {
            seen: self.seen.clone(),
            refresh: refresh.expose().to_owned(),
            renewed: Mutex::new(None),
        })
    }

    fn sign_in(&self, start: SignInStart) -> Result<FakeSignIn, ProviderError> {
        self.inner.sign_in(start)
    }

    async fn revoke(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        self.inner.revoke(account, presented).await
    }
}

impl ProviderSession for RotatingSession {
    async fn access_token(&self, _audience: &Audience) -> Result<IssuedToken, ProviderError> {
        if *self.seen.unconsented.lock().expect("unconsented") {
            return Err(ProviderError::NeedsConsent);
        }
        let n = self.seen.minted.fetch_add(1, Ordering::SeqCst) + 1;
        self.seen
            .refreshes
            .lock()
            .expect("refreshes")
            .push(self.refresh.clone());
        // The issuer takes a moment. The caller behind this one is queued by now.
        tokio::task::yield_now().await;
        let expires = UnixSeconds(NOW.0 + 3600);
        *self.renewed.lock().expect("renewed") = Some(Credential::OAuth {
            access: SecretText::new(format!("access-{n}")),
            refresh: SecretText::new(format!("refresh-{}", n + 1)),
            expires_at: expires,
            granted: Default::default(),
        });
        Ok(IssuedToken::new(
            TokenKind::Bearer,
            SecretText::new(format!("token-{n}")),
            expires,
        ))
    }

    fn renewed(&self) -> Option<Credential> {
        self.renewed.lock().expect("renewed").take()
    }
}

type Svc = AccountService<Rotating, MemorySecrets, ScriptedSheets, FixedClock, MemoryStore>;

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Mail").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

/// The mail account signed in with refresh token `refresh`, over the rotating issuer, and the
/// grant a mail app holds on it.
async fn service() -> (Svc, Seen, Candidate) {
    let seen = Seen::default();
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: mail_account().id,
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: SecretText::new("access-0"),
                refresh: SecretText::new("refresh"),
                expires_at: NOW,
                granted: Default::default(),
            },
        )
        .await
        .expect("the in-memory store cannot fail");
    let service = AccountService::new(
        vec![Rotating {
            inner: mail_provider(),
            seen: seen.clone(),
        }],
        Registry {
            accounts: vec![mail_account()],
            grants: vec![],
            toggles: vec![],
        },
        secrets,
        ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]),
        FixedClock(NOW),
    )
    .with_store(MemoryStore::default());
    let reply = service
        .handle(
            &app(),
            AccountsRequest::Choose {
                need: Need::Mail(MailNeed::new(Access::Read, true, Delta::Poll)),
                class: DataClass::Mail,
                usage: Usage::Interactive,
                window: ParentWindow::Unparented,
            },
        )
        .await;
    let AccountsReply::Chosen(candidate) = reply else {
        panic!("not chosen: {reply:?}");
    };
    (service, seen, candidate)
}

async fn token(service: &Svc, candidate: &Candidate, audience: &str) -> AccountsReply {
    service
        .handle(
            &app(),
            AccountsRequest::IssueToken {
                grant: candidate.grant.clone(),
                audience: Audience(audience.to_owned()),
            },
        )
        .await
}

fn bearer(reply: AccountsReply) -> String {
    match reply {
        AccountsReply::Token(token) => token.value.expose().to_owned(),
        other => panic!("not a token: {other:?}"),
    }
}

/// Two callers refresh one account together: the second waits for the first and uses the token
/// it got, so the issuer is called once and both hold the new token. Without the account's turn
/// both would present `refresh`, and the second would be refused by an issuer that rotates.
#[tokio::test]
async fn two_concurrent_refreshes_make_one_call_to_the_issuer() {
    let (service, seen, candidate) = service().await;
    let (first, second) = tokio::join!(
        token(&service, &candidate, "imap"),
        token(&service, &candidate, "imap"),
    );
    assert_eq!(bearer(first), "token-1");
    assert_eq!(bearer(second), "token-1");
    assert_eq!(seen.minted(), 1);
    assert_eq!(seen.refreshes(), ["refresh"]);
}

/// The rotated refresh token is stored before the access token is handed out: each refresh that
/// follows presents the one the refresh before it stored. A later call is never answered from an
/// earlier token: only calls that waited behind a refresh share it.
#[tokio::test]
async fn the_rotated_refresh_token_is_stored_before_the_next_refresh_presents_it() {
    let (service, seen, candidate) = service().await;
    assert_eq!(bearer(token(&service, &candidate, "imap").await), "token-1");
    assert_eq!(bearer(token(&service, &candidate, "smtp").await), "token-2");
    assert_eq!(bearer(token(&service, &candidate, "imap").await), "token-3");
    assert_eq!(seen.minted(), 3);
    assert_eq!(seen.refreshes(), ["refresh", "refresh-2", "refresh-3"]);
}

/// An access the account was never given is the app's refusal alone: the account stays signed
/// in, and the next token it is given is served.
#[tokio::test]
async fn a_scope_never_given_is_needs_consent_and_the_account_stays_signed_in() {
    let (service, seen, candidate) = service().await;
    *seen.unconsented.lock().expect("unconsented") = true;
    assert_eq!(
        token(&service, &candidate, "imap").await,
        AccountsReply::Refused(porter_wire::Refusal::NeedsConsent)
    );
    assert_eq!(
        service.registry().accounts[0].state,
        porter_core::AccountState::Ok
    );
    *seen.unconsented.lock().expect("unconsented") = false;
    assert_eq!(bearer(token(&service, &candidate, "imap").await), "token-1");
}
