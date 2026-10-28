//! `org.quire.Accounts1.Keys`: an app asks for the use of the person's AI keys, and is answered
//! with the permit it holds or the person's answer to the consent sheet. Nobody else asks.

use crate::launcher_register::error_name;
use crate::launcher_rig::*;

use accountd::launcher::LoginTiming;
use porter_callers::CallerRole;
use porter_core::consent::{Decision, GrantScope};
use porter_core::{AppId, AppName, Isolation};
use porter_dbus::Details;
use porter_dbus::accounts::sheet::{SheetKind, reply_of};
use porter_dbus::accounts::{KeysProxy, ServiceRoot, Sheet};
use porter_fake::Scripted;
use porter_wire::{AccountsReply, Refusal};

const DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";

fn notes() -> AppId {
    AppId {
        name: AppName::parse("org.example.Notes").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

/// `RequestKey` as an app does it, and the sheet's answer.
async fn request(connection: &zbus::Connection) -> Result<AccountsReply, zbus::Error> {
    let mut sheet = Sheet::subscribe_on(connection, ServiceRoot::Accounts)
        .await
        .expect("subscribe");
    let proxy = KeysProxy::new(connection).await.expect("proxy");
    let path = proxy
        .request_key("prompt", "interactive", "", &sheet.options())
        .await?;
    let (code, results) = sheet.response(&path).await.expect("response");
    Ok(reply_of(SheetKind::Choose, code, results).expect("a reply"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_that_holds_a_permit_is_answered_with_it_and_no_sheet_is_drawn() {
    let held = permit(
        "g-held",
        notes(),
        KEYED,
        GrantScope::Always,
        Decision::Allow,
    );
    let rig = Rig::start(vec![held], vec![], LoginTiming::new()).await;
    let app = rig.client_as("org.example.Notes", CallerRole::App).await;
    let AccountsReply::Chosen(chosen) = request(&app).await.expect("asked") else {
        panic!("the permit it holds");
    };
    assert_eq!(chosen.grant.as_str(), "g-held");
    assert!(rig.sheets.asked().is_empty(), "no sheet");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_with_no_permit_is_asked_and_the_answer_is_the_permit() {
    let rig = Rig::start(
        vec![],
        vec![Scripted::AllowFirst(GrantScope::Always)],
        LoginTiming::new(),
    )
    .await;
    let app = rig.client_as("org.example.Notes", CallerRole::App).await;
    let AccountsReply::Chosen(chosen) = request(&app).await.expect("asked") else {
        panic!("the person allowed it");
    };
    assert_eq!(rig.sheets.asked().len(), 1);
    assert!(
        rig.service
            .registry()
            .grants
            .iter()
            .any(|g| g.id == chosen.grant && g.key.app == notes()),
        "the permit is the app's"
    );
    // The same app asking again is answered with the permit, and no second sheet.
    let AccountsReply::Chosen(again) = request(&app).await.expect("asked again") else {
        panic!("the permit it holds");
    };
    assert_eq!(again.grant, chosen.grant);
    assert_eq!(rig.sheets.asked().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn after_a_dont_allow_the_app_is_refused_with_no_sheet() {
    // Don't Allow for every account that fits, as the person gave it.
    let denied = [KEYED, OAUTH, AGENT].map(|account| {
        permit(
            &format!("g-no-{account}"),
            notes(),
            account,
            GrantScope::Always,
            Decision::Deny,
        )
    });
    let rig = Rig::start(denied.to_vec(), vec![], LoginTiming::new()).await;
    let app = rig.client_as("org.example.Notes", CallerRole::App).await;
    assert_eq!(
        request(&app).await.expect("asked"),
        AccountsReply::Refused(Refusal::Denied)
    );
    assert!(rig.sheets.asked().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_app_settings_or_the_shell_may_ask() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    for (n, role) in [CallerRole::App, CallerRole::Settings, CallerRole::SheetHost]
        .into_iter()
        .enumerate()
    {
        let connection = rig.client_as(&format!("org.example.Asker{n}"), role).await;
        let proxy = KeysProxy::new(&connection).await.expect("proxy");
        // Past the role check: an unanswered ask is a Request object, not a refusal.
        assert!(
            proxy
                .request_key("prompt", "interactive", "", &Details::new())
                .await
                .is_ok(),
            "{role:?}"
        );
    }
    for role in [
        CallerRole::Agent,
        CallerRole::Cua,
        CallerRole::AgentLauncher,
        CallerRole::AiBroker,
        CallerRole::Terminal,
        CallerRole::Compositor,
        CallerRole::PorterDaemon,
    ] {
        let connection = rig.client_as("org.example.Refused", role).await;
        let proxy = KeysProxy::new(&connection).await.expect("proxy");
        assert_eq!(
            error_name(
                proxy
                    .request_key("prompt", "interactive", "", &Details::new())
                    .await
                    .expect_err("refused")
            ),
            DENIED,
            "{role:?}"
        );
    }
    let stranger = rig.bus.connect().await;
    let proxy = KeysProxy::new(&stranger).await.expect("proxy");
    assert_eq!(
        error_name(
            proxy
                .request_key("prompt", "interactive", "", &Details::new())
                .await
                .expect_err("refused")
        ),
        DENIED
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_words_are_invalid_arguments() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    let app = rig.client_as("org.example.Notes", CallerRole::App).await;
    let proxy = KeysProxy::new(&app).await.expect("proxy");
    for (class, usage) in [("sideways", "interactive"), ("prompt", "whenever")] {
        assert_eq!(
            error_name(
                proxy
                    .request_key(class, usage, "", &Details::new())
                    .await
                    .expect_err("invalid")
            ),
            "org.freedesktop.DBus.Error.InvalidArgs",
            "{class} {usage}"
        );
    }
}
