//! `org.quire.Accounts1.Broker`: what inferd asks of the daemon that keeps the AI keys. Only the
//! AI broker asks; a key leaves on a sealed memfd, once, and the audit says so without the key.

use crate::launcher_register::error_name;
use crate::launcher_rig::*;

use accountd::launcher::LoginTiming;
use porter_callers::CallerRole;
use porter_capability::{CapabilityKind, Toggle};
use porter_core::consent::{Decision, GrantScope, Verdict};
use porter_core::{AccountState, AppId, AppName, Isolation};
use porter_dbus::accounts::{BrokerProxy, verdict_from_dbus};
use porter_service_core::audit::AuditEvent;
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::io::Read;
use zbus::export::futures_core::Stream;

const DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";

fn notes() -> AppId {
    AppId {
        name: AppName::parse("org.example.Notes").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn app_arg() -> (String, String) {
    ("org.example.Notes".to_owned(), "flatpak".to_owned())
}

async fn broker(rig: &Rig) -> (zbus::Connection, BrokerProxy<'static>) {
    let connection = rig
        .client_as("org.quire.Inference", CallerRole::AiBroker)
        .await;
    let proxy = BrokerProxy::new(&connection).await.expect("proxy");
    (connection, proxy)
}

fn read_all(fd: zbus::zvariant::OwnedFd) -> String {
    let fd = std::os::fd::AsFd::as_fd(&fd)
        .try_clone_to_owned()
        .expect("a descriptor");
    assert_eq!(
        fcntl_get_seals(&fd).expect("seals"),
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
    );
    let mut text = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut text)
        .expect("read");
    text
}

fn held(id: &str, scope: GrantScope) -> porter_core::consent::Grant {
    permit(id, notes(), KEYED, scope, Decision::Allow)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_verdicts_say_what_an_apps_permits_hold_on_each_account() {
    let rig = Rig::start(
        vec![held("g-held", GrantScope::Always)],
        vec![],
        LoginTiming::new(),
    )
    .await;
    let (_connection, broker) = broker(&rig).await;
    let rows = broker
        .verdicts(&app_arg(), "prompt", "interactive")
        .await
        .expect("verdicts");
    let verdicts: Vec<_> = rows
        .into_iter()
        .map(|row| verdict_from_dbus(row).expect("a verdict"))
        .collect();
    let of = |id: &str| {
        verdicts
            .iter()
            .find(|v| v.account.as_str() == id)
            .unwrap_or_else(|| panic!("{id} in {verdicts:?}"))
    };
    assert_eq!(
        of(KEYED).verdict,
        Verdict::Granted {
            grant: porter_core::GrantId::parse("g-held").expect("id"),
            scope: GrantScope::Always
        }
    );
    assert_eq!(of(OAUTH).verdict, Verdict::Ask);
    // Another class is asked about, not granted.
    let other = broker
        .verdicts(&app_arg(), "mail", "interactive")
        .await
        .expect("verdicts");
    assert!(
        other
            .into_iter()
            .all(|row| verdict_from_dbus(row).expect("a verdict").verdict == Verdict::Ask)
    );
    assert_eq!(of(KEYED).state, AccountState::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permit_opens_the_key_on_a_sealed_descriptor_and_the_audit_says_so_without_the_key() {
    let rig = Rig::start(
        vec![held("g-held", GrantScope::Always)],
        vec![],
        LoginTiming::new(),
    )
    .await;
    let (_connection, broker) = broker(&rig).await;
    let fd = broker.read_key("g-held").await.expect("the key");
    assert_eq!(read_all(fd), KEY);
    let released: Vec<_> = rig
        .audit
        .entries()
        .into_iter()
        .filter(|entry| matches!(entry.event, AuditEvent::KeyResolved { .. }))
        .collect();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].app.as_ref(), Some(&notes()));
    assert!(!format!("{:?}", rig.audit.entries()).contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permit_that_does_not_open_a_key_is_refused_and_a_once_permit_is_spent_by_a_read() {
    let rig = Rig::start(
        vec![
            held("g-held", GrantScope::Always),
            held("g-once", GrantScope::Once),
        ],
        vec![],
        LoginTiming::new(),
    )
    .await;
    let (_connection, broker) = broker(&rig).await;
    let refused = |name: &str| format!("org.quire.Accounts1.Error.{name}");
    assert_eq!(
        error_name(broker.read_key("g-none").await.expect_err("nobody's")),
        refused("UnknownGrant")
    );
    // Turned off for language models, then on again.
    rig.service
        .set_toggle(
            &porter_core::AccountId::parse(KEYED).expect("id"),
            CapabilityKind::Llm,
            Toggle::Off,
        )
        .await
        .expect("off");
    assert_eq!(
        error_name(broker.read_key("g-held").await.expect_err("off")),
        refused("Denied")
    );
    rig.service
        .set_toggle(
            &porter_core::AccountId::parse(KEYED).expect("id"),
            CapabilityKind::Llm,
            Toggle::On,
        )
        .await
        .expect("on");
    // An allow-once permit is spent by the first read that works.
    let first = broker.read_key("g-once").await.expect("first read");
    assert_eq!(read_all(first), KEY);
    assert_eq!(
        error_name(broker.read_key("g-once").await.expect_err("spent")),
        refused("UnknownGrant")
    );
    assert!(
        broker.read_key("g-held").await.is_ok(),
        "an always permit stays"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_is_reported_as_an_account_and_the_broker_is_told_of_changes() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    let (_connection, broker) = broker(&rig).await;
    let mut changes = broker.receive_changed().await.expect("stream");
    // The broker has called, so it is known; a change made elsewhere reaches it.
    assert_eq!(broker.accounts().await.expect("accounts").len(), 3);
    rig.service
        .set_toggle(
            &porter_core::AccountId::parse(KEYED).expect("id"),
            CapabilityKind::Llm,
            Toggle::Off,
        )
        .await
        .expect("off");
    tokio::time::timeout(
        test_kit::GENEROUS,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)),
    )
    .await
    .expect("told in time")
    .expect("open");
    let accounts = broker.accounts().await.expect("accounts");
    assert_eq!(accounts.len(), 3);
    assert!(broker.servers().await.expect("servers").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_ai_broker_asks_and_every_other_role_is_refused() {
    let rig = Rig::start(
        vec![held("g-held", GrantScope::Always)],
        vec![],
        LoginTiming::new(),
    )
    .await;
    let stranger = rig.bus.connect().await;
    let mut refused = vec![stranger];
    for role in [
        CallerRole::App,
        CallerRole::Settings,
        CallerRole::SheetHost,
        CallerRole::PorterDaemon,
        CallerRole::Agent,
        CallerRole::Cua,
        CallerRole::AgentLauncher,
        CallerRole::Terminal,
        CallerRole::Compositor,
    ] {
        refused.push(rig.client_as("org.example.Other", role).await);
    }
    for connection in &refused {
        let proxy = BrokerProxy::new(connection).await.expect("proxy");
        assert_eq!(
            error_name(proxy.read_key("g-held").await.expect_err("refused")),
            DENIED
        );
        assert_eq!(
            error_name(
                proxy
                    .verdicts(&app_arg(), "prompt", "interactive")
                    .await
                    .expect_err("refused")
            ),
            DENIED
        );
        assert_eq!(
            error_name(proxy.accounts().await.expect_err("refused")),
            DENIED
        );
    }
    assert!(
        !rig.audit
            .entries()
            .iter()
            .any(|entry| matches!(entry.event, AuditEvent::KeyResolved { .. })),
        "no key was read"
    );
}
