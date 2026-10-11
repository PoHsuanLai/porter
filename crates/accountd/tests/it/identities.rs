//! `org.quire.Identities1` (lane split-identities): the identities signed in on this machine.
//! Who may call what, "Continue as" through the sheet host (and no grant from it), the one way a
//! password leaves (a descriptor, only for the pick, once, audited), the apps-using list, and
//! `IdentityRemoved`.

use crate::common;

use accountd::{Options, UsesStore};
use common::*;
use porter_core::audit::AuditEvent;
use porter_core::consent::{AskPurpose, ConsentAnswer, GrantScope};
use porter_core::sheet::{SheetInput, SheetView};
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, CapabilityKind, Identity, IdentityAddress, IdentityId, IdentityKind, IdentityName,
    ProviderId,
};
use porter_dbus::{CallerRole, IdentitiesProxy, identity_from_dbus};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
const LIMITS_EXCEEDED: &str = "org.freedesktop.DBus.Error.LimitsExceeded";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch state directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "accountd-it-identities-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn options(&self) -> Options {
        Options {
            uses: UsesStore::File(self.0.join("porter")),
            ..Options::default()
        }
    }

    fn file(&self) -> PathBuf {
        self.0.join("porter").join("identity-uses.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A host whose person picks `account` in the sheet.
fn picking(account: AccountId) -> SheetHost {
    SheetHost::answering(SheetInput::Answer(ConsentAnswer::Allow {
        account,
        scope: GrantScope::Once,
    }))
}

async fn identities(connection: &zbus::Connection) -> IdentitiesProxy<'static> {
    IdentitiesProxy::new(connection).await.expect("proxy")
}

fn storage_identity() -> Identity {
    Identity::new(
        IdentityId::parse("fake-storage").expect("id"),
        IdentityAddress("ada@cloud.invalid".into()),
        ProviderId::parse("fake-cloud").expect("provider"),
        // No provider file is loaded in the rig, so the provider's id stands in for its name.
        IdentityName("fake-cloud".into()),
        IdentityKind::Password,
    )
}

fn mail_identity() -> Identity {
    Identity::new(
        IdentityId::parse("fake-mail").expect("id"),
        IdentityAddress("ada@mail.invalid".into()),
        ProviderId::parse("fake-mail").expect("provider"),
        IdentityName("fake-mail".into()),
        IdentityKind::OAuth,
    )
}

fn settings_caller() -> porter_dbus::Caller {
    caller("org.quire.Settings", CallerRole::Settings)
}

/// The identities `List` gives, read back into values.
async fn listed(proxy: &IdentitiesProxy<'_>) -> Vec<Identity> {
    proxy
        .list()
        .await
        .expect("list")
        .iter()
        .map(|row| identity_from_dbus(row).expect("a well formed row"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn list_gives_the_sign_in_accounts_and_leaves_out_the_rest_and_every_secret() {
    let rig = Rig::start().await;
    let app = identities(&rig.client("org.quire.Photos").await).await;
    // The password account and the OAuth account, in the registry's order; the local runtime
    // (`fake-llm`) is an account but not an identity.
    assert_eq!(listed(&app).await, [storage_identity(), mail_identity()]);
    let shown = format!("{:?}", app.list().await.expect("list"));
    for secret in [APP_PASSWORD, REFRESH_TOKEN, ACCESS_TOKEN] {
        assert!(!shown.contains(secret), "{shown}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn apps_and_porter_daemons_use_identities_and_everything_else_is_refused() {
    let rig = Rig::start_with(Default::default(), picking(storage_identity_account())).await;
    let id = "fake-storage";
    // Who may ask: an app, Settings, the shell and a porter daemon. Only Settings and the shell
    // read `Uses`.
    let allowed = [
        (CallerRole::App, false),
        (CallerRole::Settings, true),
        (CallerRole::SheetHost, true),
        (CallerRole::PorterDaemon, false),
    ];
    for (n, (role, reads_uses)) in allowed.into_iter().enumerate() {
        let who = caller(&format!("org.example.Asker{n}"), role);
        let proxy = identities(&rig.client_as(who).await).await;
        assert_eq!(listed(&proxy).await.len(), 2, "{role:?} lists");
        let (picked, ..) = proxy.offer("", "").await.expect("offer");
        assert_eq!(picked, id, "{role:?} offers");
        proxy.register_use(id).await.expect("register");
        match reads_uses {
            true => assert!(proxy.uses(id).await.is_ok(), "{role:?} reads uses"),
            false => {
                let refused = proxy.uses(id).await.expect_err("uses");
                assert_eq!(error_name(&refused), ACCESS_DENIED, "{role:?}");
            }
        }
    }
    // Refused whatever they ask, and an unknown sender too (its `_` arm fails closed): nothing is
    // shown to the person.
    let opened = rig.host_log.calls().opened.len();
    let refused_roles = [
        Some(CallerRole::Agent),
        Some(CallerRole::Cua),
        Some(CallerRole::AgentLauncher),
        Some(CallerRole::Terminal),
        Some(CallerRole::Compositor),
        None,
    ];
    for role in refused_roles {
        let connection = match role {
            Some(role) => rig.client_as(caller("org.example.Refused", role)).await,
            None => rig.stranger().await,
        };
        let proxy = identities(&connection).await;
        let errors = [
            proxy.list().await.map(|_| ()).expect_err("list"),
            proxy.offer("", "").await.map(|_| ()).expect_err("offer"),
            proxy
                .take_password(id)
                .await
                .map(|_| ())
                .expect_err("take_password"),
            proxy.register_use(id).await.expect_err("register_use"),
            proxy.uses(id).await.map(|_| ()).expect_err("uses"),
        ];
        for error in errors {
            assert_eq!(error_name(&error), ACCESS_DENIED, "{role:?}");
        }
    }
    assert_eq!(rig.host_log.calls().opened.len(), opened);
}

fn storage_identity_account() -> AccountId {
    storage_account().id
}

#[tokio::test(flavor = "multi_thread")]
async fn offer_shows_the_picker_answers_the_pick_and_grants_nothing() {
    let rig = Rig::start_with(Default::default(), picking(storage_identity_account())).await;
    let app = identities(&rig.client("org.quire.Photos").await).await;
    let (id, address, provider) = app.offer("", "").await.expect("offer");
    assert_eq!(
        (id.as_str(), address.as_str(), provider.as_str()),
        ("fake-storage", "ada@cloud.invalid", "fake-cloud")
    );
    // The sheet is the one `Choose` draws, asking to continue as one of the identities (and not
    // the local runtime), for this app.
    let view = {
        let calls = rig.host_log.calls();
        assert_eq!(calls.opened.len(), 1);
        serde_json::from_str::<SheetView>(&calls.opened[0].2).expect("a view")
    };
    let SheetView::Consent(ask) = view else {
        panic!("the picker is a consent sheet: {view:?}");
    };
    assert_eq!(ask.purpose, AskPurpose::ContinueAs);
    assert_eq!(ask.kind, CapabilityKind::Identity);
    assert_eq!(ask.app, photos());
    let offered: Vec<&str> = ask.accounts.iter().map(|a| a.account.as_str()).collect();
    assert_eq!(offered, ["fake-storage", "fake-mail"]);
    // Nothing was granted, and no token went anywhere: the answer is the three words above.
    let registry = rig.service.registry();
    assert!(registry.grants.is_empty());
    // The sheet was taken down once the person had picked.
    eventually("the picker to be taken down", || {
        rig.host_log.calls().closed.len() == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hint_lists_only_that_providers_identities() {
    let rig = Rig::start_with(Default::default(), picking(porter_fake::mail_account().id)).await;
    let app = identities(&rig.client("org.quire.Photos").await).await;
    let (id, ..) = app.offer("fake-mail", "").await.expect("offer");
    assert_eq!(id, "fake-mail");
    let calls = rig.host_log.calls();
    let SheetView::Consent(ask) = serde_json::from_str(&calls.opened[0].2).expect("a view") else {
        panic!("a consent sheet");
    };
    let offered: Vec<&str> = ask.accounts.iter().map(|a| a.account.as_str()).collect();
    assert_eq!(offered, ["fake-mail"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_offer_the_person_does_not_answer_with_a_pick_is_dismissed() {
    let dismissed = refusal_name(Refusal::Dismissed);
    let inputs = [
        SheetInput::Dismiss,
        SheetInput::Answer(ConsentAnswer::Deny),
        SheetInput::Answer(ConsentAnswer::AddAccount),
        // An answer for an account the sheet did not offer is a dismissal, as `Choose` reads it.
        SheetInput::Answer(ConsentAnswer::Allow {
            account: porter_fake::llm_account().id,
            scope: GrantScope::Once,
        }),
    ];
    for input in inputs {
        let rig = Rig::start_with(Default::default(), SheetHost::answering(input.clone())).await;
        let app = identities(&rig.client("org.quire.Photos").await).await;
        let refused = app.offer("", "").await.expect_err("no pick");
        assert_eq!(error_name(&refused), dismissed, "{input:?}");
        // No pick, so no password.
        let taken = app.take_password("fake-storage").await.expect_err("take");
        assert_eq!(error_name(&taken), ACCESS_DENIED, "{input:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hint_that_fits_no_identity_or_is_no_provider_shows_nothing() {
    let rig = Rig::start().await;
    let app = identities(&rig.client("org.quire.Photos").await).await;
    let none = app
        .offer("no-such-provider", "")
        .await
        .expect_err("none fit");
    assert_eq!(error_name(&none), refusal_name(Refusal::NoFittingAccount));
    // The local runtime's provider has an account but no identity.
    let runtime = app
        .offer("fake-llm", "")
        .await
        .expect_err("not an identity");
    assert_eq!(
        error_name(&runtime),
        refusal_name(Refusal::NoFittingAccount)
    );
    let bad = app.offer("Not A Provider", "").await.expect_err("bad hint");
    assert_eq!(error_name(&bad), INVALID_ARGS);
    assert!(rig.host_log.calls().opened.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_password_goes_on_a_descriptor_to_the_app_the_person_picked_it_for_once() {
    let rig = Rig::start_with(Default::default(), picking(storage_identity_account())).await;
    let mut tap = Tap::start(&rig.bus).await;
    let client = rig.client("org.quire.Photos").await;
    let app = identities(&client).await;
    // Nobody has picked yet: refused, from this connection and from another of the same app.
    let other = identities(&rig.client("org.quire.Photos").await).await;
    for proxy in [&app, &other] {
        let early = proxy
            .take_password("fake-storage")
            .await
            .expect_err("early");
        assert_eq!(error_name(&early), ACCESS_DENIED);
    }
    app.offer("", "").await.expect("offer");
    // Another connection of the same app did not get the pick; neither did another identity.
    let stolen = other
        .take_password("fake-storage")
        .await
        .expect_err("other");
    assert_eq!(error_name(&stolen), ACCESS_DENIED);
    let wrong = app.take_password("fake-mail").await.expect_err("wrong");
    assert_eq!(error_name(&wrong), ACCESS_DENIED);

    let fd = app.take_password("fake-storage").await.expect("take");
    let mut text = String::new();
    std::fs::File::from(std::os::fd::OwnedFd::from(fd))
        .read_to_string(&mut text)
        .expect("readable from the start");
    assert_eq!(text, APP_PASSWORD);
    // Once: the pick is spent.
    let again = app.take_password("fake-storage").await.expect_err("again");
    assert_eq!(error_name(&again), ACCESS_DENIED);

    // One audit line: which app, which account; never the password.
    let handed: Vec<_> = rig
        .audit
        .entries()
        .into_iter()
        .filter(|e| e.event == AuditEvent::PasswordHandedOver)
        .collect();
    assert_eq!(handed.len(), 1, "{:?}", rig.audit.entries());
    assert_eq!(handed[0].app.as_ref(), Some(&photos()));
    assert_eq!(handed[0].account, Some(storage_identity_account()));
    // No message on the bus, in any call, answer or signal, carries it.
    for message in tap.drain().await {
        assert!(
            !contains(&message, APP_PASSWORD),
            "the password is on the bus"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_identity_that_signs_in_with_oauth_has_no_password_to_hand_over() {
    let rig = Rig::start_with(Default::default(), picking(porter_fake::mail_account().id)).await;
    let mut tap = Tap::start(&rig.bus).await;
    let app = identities(&rig.client("org.quire.Mail").await).await;
    let (id, ..) = app.offer("", "").await.expect("offer");
    assert_eq!(id, "fake-mail");
    // Picked, and still nothing goes: there is no password, and the refresh token never leaves.
    let refused = app.take_password("fake-mail").await.expect_err("oauth");
    assert_eq!(error_name(&refused), ACCESS_DENIED);
    assert!(
        !rig.audit
            .entries()
            .iter()
            .any(|e| e.event == AuditEvent::PasswordHandedOver)
    );
    for message in tap.drain().await {
        for secret in [REFRESH_TOKEN, ACCESS_TOKEN] {
            assert!(!contains(&message, secret), "{secret} is on the bus");
        }
    }
    // An identity that is not there is an invalid argument, not a refusal to hand over.
    let gone = app.take_password("not-there").await.expect_err("gone");
    assert_eq!(error_name(&gone), INVALID_ARGS);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_has_one_picker_open_at_a_time_and_leaving_takes_it_down() {
    let rig = Rig::start().await;
    let first = rig.client("org.quire.Photos").await;
    let waiting = {
        let proxy = identities(&first).await;
        tokio::spawn(async move { proxy.offer("", "").await })
    };
    eventually("the picker to be shown", || {
        rig.host_log.calls().opened.len() == 1
    })
    .await;
    // The same app, from another connection, cannot stack a second sheet on the person.
    let second = identities(&rig.client("org.quire.Photos").await).await;
    let busy = second.offer("", "").await.expect_err("one at a time");
    assert_eq!(error_name(&busy), LIMITS_EXCEEDED);
    // The asker leaves: the sheet is taken down, and nobody is left waiting for an answer.
    first.close().await.expect("close");
    waiting.abort();
    eventually("the picker to be taken down", || {
        rig.host_log.calls().closed.len() == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apps_that_signed_in_are_listed_for_settings_and_go_with_the_identity() {
    let scratch = Scratch::new();
    let rig = Rig::start_with(scratch.options(), SheetHost::quiet()).await;
    let photos = identities(&rig.client("org.quire.Photos").await).await;
    let mail = identities(&rig.client("org.quire.Mail").await).await;
    photos.register_use("fake-storage").await.expect("register");
    photos
        .register_use("fake-storage")
        .await
        .expect("saying it twice changes nothing");
    mail.register_use("fake-storage").await.expect("register");
    mail.register_use("fake-mail").await.expect("register");
    let unknown = mail.register_use("not-there").await.expect_err("unknown");
    assert_eq!(error_name(&unknown), INVALID_ARGS);

    let settings = identities(&rig.client_as(settings_caller()).await).await;
    assert_eq!(
        settings.uses("fake-storage").await.expect("uses"),
        ["org.quire.Mail", "org.quire.Photos"]
    );
    assert_eq!(
        settings.uses("fake-mail").await.expect("uses"),
        ["org.quire.Mail"]
    );
    let denied = photos.uses("fake-storage").await.expect_err("an app");
    assert_eq!(error_name(&denied), ACCESS_DENIED);
    let gone = settings.uses("not-there").await.expect_err("unknown");
    assert_eq!(error_name(&gone), INVALID_ARGS);

    // Kept beside registry.json, and across a start.
    let saved = std::fs::read_to_string(scratch.file()).expect("the uses file");
    assert!(
        saved.contains("org.quire.Photos") && saved.contains("fake-mail"),
        "{saved}"
    );
    let again = Rig::start_with(scratch.options(), SheetHost::quiet()).await;
    let settings_again = identities(&again.client_as(settings_caller()).await).await;
    assert_eq!(
        settings_again.uses("fake-storage").await.expect("uses"),
        ["org.quire.Mail", "org.quire.Photos"]
    );

    // Settings removes the identity: every app that has called is told, once, and its list goes.
    let mut removed = photos.receive_identity_removed().await.expect("subscribe");
    let account = common::settings(&rig).await;
    account
        .invoke(&common::key("accounts.fake-storage.remove"))
        .await
        .expect("removed");
    let told = tokio::time::timeout(
        porter_fake::GENEROUS,
        std::future::poll_fn(|cx| {
            zbus::export::futures_core::Stream::poll_next(std::pin::Pin::new(&mut removed), cx)
        }),
    )
    .await
    .expect("told")
    .expect("a signal");
    assert_eq!(told.args().expect("args").identity_id, "fake-storage");
    let after = settings.uses("fake-storage").await.expect_err("removed");
    assert_eq!(error_name(&after), INVALID_ARGS);
    assert_eq!(listed(&photos).await, [mail_identity()]);
    let saved = std::fs::read_to_string(scratch.file()).expect("the uses file");
    assert!(!saved.contains("fake-storage"), "{saved}");
    assert!(saved.contains("fake-mail"), "{saved}");
}
