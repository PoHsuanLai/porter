//! `BusSheets` against a sheet host written by hand on the private bus: only the verified host
//! is dialled, only its `Input` for the right handle counts, and a host that leaves ends the
//! sheet. The forged-signal tests are ported from the caller side's (`porter-dbus`).

use crate::common;

use accountd::BusSheets;
use common::host::send_input;
use common::*;
use porter_core::SecretText;
use porter_core::consent::{ConsentAnswer, GrantScope};
use porter_core::sheet::{FieldAnswer, FieldKind, FieldValue, SheetInput, SheetView};
use porter_core::wire::ParentWindow;
use porter_dbus::{CallerRole, ManagerProxy, Sheet};
use porter_service::{SheetFault, SheetLink, SheetOpen, Sheets};
use std::sync::Arc;
use std::time::Duration;

fn allow() -> SheetInput {
    SheetInput::Answer(ConsentAnswer::Allow {
        account: storage_account().id,
        scope: GrantScope::Always,
    })
}

/// Starts `Choose` as photos and returns its Request path, response listener and the handle the
/// host was opened with.
async fn choosing(
    rig: &Rig,
) -> (
    zbus::Connection,
    Sheet,
    zbus::zvariant::OwnedObjectPath,
    String,
) {
    let client = rig.client("org.quire.Photos").await;
    let sheet = Sheet::subscribe(&client).await.expect("subscribe");
    let path = ManagerProxy::new(&client)
        .await
        .expect("proxy")
        .choose(
            &storage_need(),
            "photos",
            "interactive",
            "wayland:abc",
            &sheet.options(),
        )
        .await
        .expect("choose");
    eventually("the host to be opened", || {
        !rig.host_log.calls().opened.is_empty()
    })
    .await;
    let handle = rig.host_log.calls().opened[0].0.clone();
    (client, sheet, path, handle)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_consent_is_shown_with_the_window_and_answered_by_the_host() {
    let rig = Rig::start_with(Default::default(), SheetHost::quiet()).await;
    let (_client, mut sheet, path, handle) = choosing(&rig).await;
    {
        let calls = rig.host_log.calls();
        assert_eq!(calls.opened[0].1, "wayland:abc");
        let view: SheetView = serde_json::from_str(&calls.opened[0].2).expect("a view");
        assert!(matches!(view, SheetView::Consent(_)));
    }
    send_input(&rig.host_connection, &handle, &allow()).await;
    let (code, results) = sheet.response(&path).await.expect("response");
    assert_eq!(code, 0, "{results:?}");
    // The sheet was taken down when the answer came.
    eventually("the host to be told to close", || {
        rig.host_log.calls().closed == vec![handle.clone()]
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forged_input_is_ignored_whoever_sends_it_and_whatever_handle_it_names() {
    let rig = Rig::start_with(Default::default(), SheetHost::quiet()).await;
    let (_client, mut sheet, path, handle) = choosing(&rig).await;

    // Another connection sends an Allow for the real handle.
    let forger = rig.stranger().await;
    send_input(&forger, &handle, &allow()).await;
    // The real host sends an Allow for a handle that is not this sheet's.
    send_input(&rig.host_connection, "accountd-999", &allow()).await;
    // And a line that is not an input at all.
    let emitter =
        zbus::object_server::SignalEmitter::new(&rig.host_connection, porter_dbus::SHEET_PATH)
            .expect("emitter");
    emitter
        .emit(
            "org.quire.AccountsSheet1",
            "Input",
            &(handle.as_str(), "{not json"),
        )
        .await
        .expect("emit");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        rig.service.registry().grants.is_empty(),
        "no grant from a forgery"
    );

    // Only the owner's answer for the handle counts: it refuses.
    send_input(
        &rig.host_connection,
        &handle,
        &SheetInput::Answer(ConsentAnswer::Deny),
    )
    .await;
    let (code, results) = sheet.response(&path).await.expect("response");
    assert_eq!(code, 2, "{results:?}");
    assert_eq!(text_of(&results, "refusal").as_deref(), Some("denied"));
    assert!(
        rig.service
            .registry()
            .grants
            .iter()
            .all(|g| g.decision == porter_core::consent::Decision::Deny)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_that_only_claimed_the_name_is_not_the_host() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let host_name = rig.host_connection.unique_name().expect("name").to_string();
    // The owner of the name is not in the SheetHost role.
    rig.callers
        .introduce_as(&host_name, caller("org.example.Squatter", CallerRole::App));
    let client = rig.client("org.quire.Photos").await;
    let (code, _) = choose(&client).await;
    assert_eq!(
        code, 1,
        "the sheet could not be shown, so nothing was allowed"
    );
    assert!(
        rig.host_log.calls().opened.is_empty(),
        "the squatter saw nothing"
    );
    assert!(rig.service.registry().grants.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_host_on_the_bus_is_no_sheet() {
    let rig = Rig::start_with(Default::default(), SheetHost::quiet()).await;
    rig.host_connection
        .release_name(porter_dbus::SHEET_BUS)
        .await
        .expect("release");
    let client = rig.client("org.quire.Photos").await;
    let (code, _) = choose(&client).await;
    assert_eq!(code, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_that_leaves_ends_the_sheet() {
    let rig = Rig::start_with(Default::default(), SheetHost::quiet()).await;
    let (_client, mut sheet, path, _handle) = choosing(&rig).await;
    rig.host_connection.close().await.expect("close");
    let (code, _) = sheet.response(&path).await.expect("response");
    assert_eq!(code, 1, "no answer is a dismissal");
    assert!(rig.service.registry().grants.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_that_closes_the_request_takes_the_host_sheet_down() {
    let rig = Rig::start_with(Default::default(), SheetHost::quiet()).await;
    let (_client, sheet, path, handle) = choosing(&rig).await;
    sheet.closer(Some(path)).expect("closer").close().await;
    eventually("the host to be told to close", || {
        rig.host_log.calls().closed == vec![handle.clone()]
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_conversation_carries_views_out_and_typed_secrets_in() {
    let rig = Rig::start().await;
    let sheets = BusSheets::new(rig.connection.clone(), Arc::clone(&rig.callers));
    let mut link = sheets
        .conversation(SheetOpen {
            window: ParentWindow::Unparented,
            view: SheetView::Done,
        })
        .await
        .expect("open");
    let handle = rig.host_log.calls().opened[0].0.clone();
    link.update(SheetView::Working {
        provider: porter_core::ProviderId::parse("fake-cloud").expect("id"),
        row: None,
    })
    .await
    .expect("update");
    assert_eq!(rig.host_log.calls().updated.len(), 1);

    let submit = SheetInput::Submit(vec![FieldAnswer {
        kind: FieldKind::Password,
        value: FieldValue::Secret(SecretText::new(APP_PASSWORD)),
    }]);
    send_input(&rig.host_connection, &handle, &submit).await;
    let got = link.input().await.expect("input");
    assert_eq!(got, submit);
    assert!(
        !format!("{got:?}").contains(APP_PASSWORD),
        "Debug redacts the secret"
    );

    drop(link);
    eventually("the sheet to be closed on drop", || {
        rig.host_log.calls().closed == vec![handle.clone()]
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_reports_closed_when_the_host_goes() {
    let rig = Rig::start().await;
    let sheets = BusSheets::new(rig.connection.clone(), Arc::clone(&rig.callers));
    let mut link = sheets
        .conversation(SheetOpen {
            window: ParentWindow::Unparented,
            view: SheetView::Done,
        })
        .await
        .expect("open");
    rig.host_connection.close().await.expect("close");
    assert_eq!(link.input().await, Err(SheetFault::Closed));
    assert_eq!(link.update(SheetView::Done).await, Err(SheetFault::Closed));
}

#[tokio::test(flavor = "multi_thread")]
async fn add_account_on_the_alert_opens_the_add_sheet_and_cancelling_it_grants_nothing() {
    let rig = Rig::start_with(Default::default(), SheetHost::quiet()).await;
    let (_client, mut sheet, path, handle) = choosing(&rig).await;
    // The wire slug sill sends.
    assert_eq!(
        serde_json::to_string(&SheetInput::Answer(ConsentAnswer::AddAccount)).expect("json"),
        r#"{"kind":"answer","v":{"kind":"add_account"}}"#
    );
    send_input(
        &rig.host_connection,
        &handle,
        &SheetInput::Answer(ConsentAnswer::AddAccount),
    )
    .await;
    // The alert is taken down and the add sheet is opened for the same window.
    eventually("the add sheet to be opened", || {
        rig.host_log.calls().opened.len() == 2
    })
    .await;
    let (add_handle, window, view) = rig.host_log.calls().opened[1].clone();
    assert_ne!(add_handle, handle);
    assert_eq!(window, "wayland:abc");
    let view: SheetView = serde_json::from_str(&view).expect("a view");
    assert!(
        matches!(view, SheetView::Providers(_)),
        "the provider list: {view:?}"
    );
    // Cancel the add: nothing is granted and the app is told it was dismissed.
    send_input(&rig.host_connection, &add_handle, &SheetInput::Dismiss).await;
    let (code, results) = sheet.response(&path).await.expect("response");
    assert_eq!(code, 1, "{results:?}");
    assert!(rig.service.registry().grants.is_empty());
}

/// rel-11: an app has one sheet of a kind open at a time. A second ask while the first is open
/// is refused at once (`LimitsExceeded`, no sheet, no Request object), from another connection of
/// the same app too; once the first is closed the app may ask again.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_sheet_of_a_kind_while_one_is_open_is_refused_until_it_ends() {
    let rig = Rig::start_with(Default::default(), SheetHost::quiet()).await;
    let (client, sheet, path, _handle) = choosing(&rig).await;
    let ask = |connection: zbus::Connection| async move {
        let options = Sheet::subscribe(&connection)
            .await
            .expect("subscribe")
            .options();
        ManagerProxy::new(&connection)
            .await
            .expect("proxy")
            .choose(
                &storage_need(),
                "photos",
                "interactive",
                "wayland:abc",
                &options,
            )
            .await
    };
    let again = ask(client.clone()).await.expect_err("one is open");
    assert_eq!(
        error_name(&again),
        "org.freedesktop.DBus.Error.LimitsExceeded"
    );
    let other_connection = rig.client("org.quire.Photos").await;
    let again = ask(other_connection).await.expect_err("the same app");
    assert_eq!(
        error_name(&again),
        "org.freedesktop.DBus.Error.LimitsExceeded"
    );
    assert_eq!(rig.host_log.calls().opened.len(), 1, "no second sheet");
    // Another app is not held up.
    ask(rig.client("org.quire.Mail").await)
        .await
        .expect("another app's sheet");

    sheet.closer(Some(path)).expect("closer").close().await;
    let mut asked = None;
    for _ in 0..200 {
        match ask(client.clone()).await {
            Ok(path) => {
                asked = Some(path);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    assert!(
        asked.is_some(),
        "the app may ask again once its sheet ended"
    );
}
