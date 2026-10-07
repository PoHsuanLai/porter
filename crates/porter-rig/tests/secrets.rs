//! `porter-rig-secrets` as a process: a fake `org.freedesktop.secrets` on a private bus, and
//! accountd's own `Oo7Secrets` (the real client, session negotiation and all) storing,
//! refusing to overwrite, reading back and deleting through it, from a child process whose
//! only bus is the private one. The service ends cleanly on SIGTERM sent to its PID.

mod common;

use common::bus::PrivateBus;
use common::{Proc, scratch};
use porter_core::{AccountId, Credential, SecretKey, SecretPurpose, SecretText, UnixSeconds};
use porter_secrets::{Oo7Secrets, PutOutcome, Secrets, SecretsError};
use std::process::{Command, Stdio};

const CHILD: &str = "RIG_OO7_CHILD";

fn key(account: &str) -> SecretKey {
    SecretKey {
        account: AccountId::parse(account).expect("id"),
        purpose: SecretPurpose::Password,
    }
}

fn credential(refresh: &str) -> Credential {
    Credential::OAuth {
        access: SecretText::new("an-access-token"),
        refresh: SecretText::new(refresh),
        expires_at: UnixSeconds(5),
    }
}

/// The client half: runs only in the child process the other test starts, where the session
/// bus is the private one. Anywhere else it is a test of nothing and passes.
#[tokio::test(flavor = "multi_thread")]
async fn oo7_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let store = Oo7Secrets;
    let one = key("rig-account");
    assert_eq!(store.get(&one).await, Err(SecretsError::Missing));
    store
        .put(&one, &credential("RIG-REFRESH-1"))
        .await
        .expect("put");
    assert_eq!(
        store.get(&one).await.expect("get"),
        credential("RIG-REFRESH-1")
    );
    assert_eq!(
        store
            .put_if_absent(&one, &credential("RIG-REFRESH-2"))
            .await,
        Ok(PutOutcome::AlreadyThere)
    );
    assert_eq!(
        store.get(&one).await.expect("get"),
        credential("RIG-REFRESH-1")
    );
    store
        .put(&one, &credential("RIG-REFRESH-3"))
        .await
        .expect("replace");
    assert_eq!(
        store.get(&one).await.expect("get"),
        credential("RIG-REFRESH-3")
    );
    let two = key("other-account");
    assert_eq!(
        store
            .put_if_absent(&two, &credential("RIG-REFRESH-4"))
            .await,
        Ok(PutOutcome::Stored)
    );
    store.delete(&one).await.expect("delete");
    assert_eq!(store.get(&one).await, Err(SecretsError::Missing));
    store
        .delete_account(&AccountId::parse("other-account").expect("id"))
        .await
        .expect("wipe");
    assert_eq!(store.get(&two).await, Err(SecretsError::Missing));
}

#[tokio::test(flavor = "multi_thread")]
async fn accountds_oo7_secrets_stores_and_reads_back_through_the_fake_and_it_ends_on_sigterm() {
    let bus = PrivateBus::start();
    let home = scratch("secrets");
    let mut service = Proc::spawn(
        env!("CARGO_BIN_EXE_porter-rig-secrets"),
        &[],
        &[
            (
                "DBUS_SESSION_BUS_ADDRESS",
                std::path::Path::new(bus.address()),
            ),
            ("HOME", &home),
            ("XDG_RUNTIME_DIR", &home),
        ],
    );
    let probe = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&probe).await.expect("proxy");
    let name: zbus::names::BusName<'_> = "org.freedesktop.secrets".try_into().expect("name");
    let mut taken = false;
    for _ in 0..1500 {
        if dbus.name_has_owner(name.clone()).await.unwrap_or(false) {
            taken = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(taken, "the service takes org.freedesktop.secrets");

    let exe = std::env::current_exe().expect("this test binary");
    let output = tokio::task::spawn_blocking({
        let (address, home) = (bus.address().to_owned(), home.clone());
        move || {
            Command::new(exe)
                .env_clear()
                .env(CHILD, "1")
                .env("DBUS_SESSION_BUS_ADDRESS", address)
                .env("HOME", &home)
                .env("XDG_RUNTIME_DIR", &home)
                .args(["--exact", "oo7_child", "--nocapture", "--test-threads=1"])
                .stdin(Stdio::null())
                .output()
                .expect("the child test runs")
        }
    })
    .await
    .expect("join");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{said}");
    assert!(said.contains("1 passed"), "the child test ran: {said}");

    let status = service.terminate();
    assert!(status.success(), "{status:?}");
    let _ = std::fs::remove_dir_all(home);
}
