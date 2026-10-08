//! An app's own Space belongs to that app (lane spaces): a grant over another app's own Space is
//! refused `Denied` when the app uses it, checked against the calling app (the service's
//! `caller`, which the transport verified), never the Space id's text alone. A grant over a
//! desktop-wide Space, outside any Space, or any Space is used as before.

use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::wire::Refusal;
use porter_core::{
    AccountsReply, AccountsRequest, AppId, AppName, Audience, CapabilityKind, Credential,
    DataClass, GrantId, Isolation, SecretKey, SecretPurpose, SecretText, SpaceId, SpaceScope,
    UnixSeconds,
};
use porter_fake::{
    FakeProvider, FixedClock, MemoryStore, RecordingAudit, ScriptedSheets, mail_account,
    mail_provider,
};
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};

type Svc = AccountService<
    FakeProvider,
    MemorySecrets,
    ScriptedSheets,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

fn mail_app() -> AppId {
    AppId {
        name: AppName::parse("org.example.Mail").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn grant(id: &str, space: SpaceScope) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("id"),
        key: GrantKey {
            app: mail_app(),
            account: mail_account().id,
            kind: CapabilityKind::Mail,
            class: DataClass::Mail,
            usage: Usage::Interactive,
            space,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }
}

fn only(space: &str) -> SpaceScope {
    SpaceScope::Only(SpaceId::parse(space).expect("space"))
}

/// The mail account and one allowing grant per Space kind for `org.example.Mail`; `theirs` is
/// over Photos' own Space, as if a store had been written by hand.
async fn service() -> Svc {
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
    let grants = vec![
        grant("theirs", only("app:org.quire.Photos:3")),
        grant("own", only("app:org.example.Mail:1")),
        grant("linked", only("work")),
        grant("outside", only("desktop")),
        grant("any", SpaceScope::Any),
    ];
    AccountService::new(
        vec![mail_provider()],
        Registry {
            accounts: vec![account],
            grants,
            ..Registry::default()
        },
        secrets,
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    )
    .with_store(MemoryStore::default())
    .with_audit(RecordingAudit::default())
}

async fn token(service: &Svc, grant: &str) -> AccountsReply {
    service
        .handle(
            &mail_app(),
            AccountsRequest::IssueToken {
                grant: GrantId::parse(grant).expect("id"),
                audience: Audience("imap".into()),
            },
        )
        .await
}

#[tokio::test]
async fn a_grant_over_another_apps_own_space_is_refused() {
    let service = service().await;
    assert_eq!(
        token(&service, "theirs").await,
        AccountsReply::Refused(Refusal::Denied)
    );
    let imap = mail_account().endpoints[0].url.clone();
    let relay = service
        .open_authenticated(&mail_app(), &GrantId::parse("theirs").expect("id"), &imap)
        .await;
    assert_eq!(relay.map(|_| ()), Err(Refusal::Denied));
}

#[tokio::test]
async fn own_linked_outside_and_any_spaces_still_pass() {
    let service = service().await;
    for grant in ["own", "linked", "outside", "any"] {
        let reply = token(&service, grant).await;
        assert!(
            matches!(reply, AccountsReply::Token(_)),
            "{grant}: {reply:?}"
        );
    }
}
