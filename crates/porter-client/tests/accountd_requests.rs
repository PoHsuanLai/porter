//! accountd's side of the Request objects, with raw proxies instead of the client: where the
//! object is, who may close it, who hears the answer, and what ends a sheet.
#![cfg(feature = "dbus")]

mod common;

use accountd::Host;
use common::accountd::{Daemon, photos};
use common::bus::PrivateBus;
use common::eventually;
use porter_core::Need;
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::need::StorageNeed;
use porter_core::wire::Refusal;
use porter_core::{AccountsReply, AccountsRequest, AppId};
use porter_dbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use porter_dbus::{
    BusError, Details, ManagerProxy, RequestProxy, Sheet, SheetKind, need_to_dbus, reply_of,
    request_namespace, request_path,
};
use porter_fake::Scripted;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use zbus::Connection;

fn need() -> porter_dbus::NeedArg {
    need_to_dbus(&Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    }))
}

fn token(text: &str) -> Details {
    Details::from([(
        "handle_token".to_owned(),
        OwnedValue::try_from(Value::from(text.to_owned())).expect("value"),
    )])
}

async fn choose(client: &Connection, options: &Details) -> Result<OwnedObjectPath, BusError> {
    ManagerProxy::new(client)
        .await?
        .choose(&need(), "photos", "interactive", "", options)
        .await
}

fn unique(client: &Connection) -> String {
    client.unique_name().expect("name").to_string()
}

fn error_name(error: &BusError) -> String {
    match error {
        BusError::MethodError(name, _, _) => name.to_string(),
        other => format!("{other:?}"),
    }
}

async fn quiet<T>(what: impl Future<Output = T>) -> bool {
    tokio::time::timeout(Duration::from_millis(300), what)
        .await
        .is_err()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_request_object_is_where_the_token_says_and_the_answer_goes_to_the_caller_alone() {
    let daemon = Daemon::start([Scripted::Dismiss]).await;
    let client = daemon.client().await;
    let spy = daemon.stranger().await;
    let spy_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.quire.Accounts1.Request")
        .expect("interface")
        .build();
    let mut overheard = zbus::MessageStream::for_match_rule(spy_rule, &spy, None)
        .await
        .expect("spy subscribes");

    let mut sheet = Sheet::subscribe(&client).await.expect("subscribe");
    let path = choose(&client, &sheet.options()).await.expect("path");
    assert_eq!(Some(path.clone()), sheet.expected_path());
    assert!(
        path.as_str()
            .starts_with(&request_namespace(&unique(&client)))
    );

    let (code, results) = sheet.response(&path).await.expect("response");
    assert_eq!(
        reply_of(SheetKind::Choose, code, results),
        Ok(AccountsReply::Refused(Refusal::Dismissed))
    );
    assert_eq!(code, 1, "closing the sheet is code 1");
    use zbus::export::futures_core::Stream;
    let heard = quiet(std::future::poll_fn(|cx| {
        std::pin::Pin::new(&mut overheard).poll_next(cx)
    }))
    .await;
    assert!(heard, "another connection hears nothing of it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_with_no_token_gets_a_path_under_its_callers_namespace() {
    let daemon = Daemon::start([Scripted::Dismiss, Scripted::Dismiss]).await;
    let client = daemon.client().await;
    let one = choose(&client, &Details::new()).await.expect("path");
    let two = choose(&client, &Details::new()).await.expect("path");
    let namespace = request_namespace(&unique(&client));
    assert!(one.as_str().starts_with(&namespace), "{one}");
    assert!(two.as_str().starts_with(&namespace), "{two}");
    assert_ne!(one, two);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_that_is_not_a_path_segment_or_is_in_use_is_refused() {
    let daemon = Daemon::start([Scripted::Hang]).await;
    let client = daemon.client().await;
    for bad in ["has/slash", "has.dot", ""] {
        let got = choose(&client, &token(bad)).await;
        assert_eq!(
            got.map_err(|e| error_name(&e)),
            Err("org.freedesktop.DBus.Error.InvalidArgs".to_owned()),
            "{bad:?}"
        );
    }
    let first = choose(&client, &token("mine")).await.expect("the first");
    assert_eq!(
        first.as_str(),
        request_path(&unique(&client), "mine").expect("path")
    );
    let again = choose(&client, &token("mine")).await;
    assert_eq!(
        again.map_err(|e| error_name(&e)),
        Err("org.freedesktop.DBus.Error.InvalidArgs".to_owned()),
        "the token is in use while its sheet is open"
    );
    // Closing the sheet frees it.
    let proxy = RequestProxy::builder(&client)
        .path(first.clone())
        .expect("path")
        .build()
        .await
        .expect("proxy");
    proxy.close().await.expect("close");
    eventually("the object to go", || daemon.asked.abandoned() == 1).await;
    let mut free = false;
    for _ in 0..200 {
        if choose(&client, &token("mine")).await.is_ok() {
            free = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(free, "the token is free once the object is gone");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_caller_may_close_its_sheet_and_a_closed_sheet_never_answers() {
    let daemon = Daemon::start([Scripted::Hang]).await;
    let client = daemon.client().await;
    let other = daemon.client().await;
    let mut sheet = Sheet::subscribe(&client).await.expect("subscribe");
    let path = choose(&client, &sheet.options()).await.expect("path");
    eventually("the sheet to be shown", || daemon.asked.asked().len() == 1).await;

    let build = |connection: &Connection| {
        let connection = connection.clone();
        let path = path.clone();
        async move {
            RequestProxy::builder(&connection)
                .path(path)
                .expect("path")
                .build()
                .await
                .expect("proxy")
        }
    };
    let refused = build(&other).await.close().await.expect_err("not theirs");
    assert_eq!(
        error_name(&refused),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    assert_eq!(daemon.asked.abandoned(), 0, "the sheet stays open");

    build(&client)
        .await
        .close()
        .await
        .expect("the caller closes");
    eventually("the sheet to be taken down", || {
        daemon.asked.abandoned() == 1
    })
    .await;
    assert!(
        quiet(sheet.response(&path)).await,
        "no Response follows a Close"
    );
    let gone = build(&client)
        .await
        .close()
        .await
        .expect_err("the object is gone");
    assert_eq!(
        error_name(&gone),
        "org.freedesktop.DBus.Error.UnknownObject"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_that_leaves_the_bus_takes_its_sheet_with_it() {
    let daemon = Daemon::start([Scripted::Hang]).await;
    let client = daemon.client().await;
    choose(&client, &Details::new()).await.expect("path");
    eventually("the sheet to be shown", || daemon.asked.asked().len() == 1).await;
    client.close().await.expect("leaves");
    eventually("the sheet to be taken down", || {
        daemon.asked.abandoned() == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_caller_gets_no_request_object() {
    let daemon = Daemon::start([Scripted::Dismiss]).await;
    let stranger = daemon.stranger().await;
    let got = choose(&stranger, &Details::new()).await;
    assert_eq!(
        got.map_err(|e| error_name(&e)),
        Err("org.freedesktop.DBus.Error.AccessDenied".to_owned())
    );
    assert!(daemon.asked.asked().is_empty());
}

/// A service whose call panics: the caller must be answered, not left waiting on a dead task.
#[derive(Debug)]
struct Panics;

impl Host for Panics {
    async fn handle(&self, _caller: &AppId, _request: AccountsRequest) -> AccountsReply {
        panic!("a service that cannot answer");
    }

    fn registry(&self) -> porter_service::Registry {
        porter_service::Registry::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_that_panics_answers_unavailable() {
    let bus = PrivateBus::start();
    let daemon = bus.connect().await;
    let callers = Arc::new(accountd::TableCallers::new());
    accountd::serve(&daemon, Arc::new(Panics), Arc::clone(&callers))
        .await
        .expect("serves");
    let client = bus.connect().await;
    callers.introduce(&unique(&client), photos());
    let mut sheet = Sheet::subscribe(&client).await.expect("subscribe");
    let path = choose(&client, &sheet.options()).await.expect("path");
    let (code, results) = sheet.response(&path).await.expect("an answer");
    assert_eq!(
        reply_of(SheetKind::Choose, code, results),
        Ok(AccountsReply::Refused(Refusal::Unavailable))
    );
}
