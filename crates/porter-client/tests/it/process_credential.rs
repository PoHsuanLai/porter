//! A key for a process the launcher spawns, through `porter-client`'s `Launcher`, against a
//! hand-written accountd (`common::fake_accountd`) on a private bus: issue on a memfd, set a
//! child up with it, end it, and hear accountd end it. What accountd checks before it issues one
//! (the permit, the program, the caller) is in accountd's launcher tests.
#![cfg(feature = "dbus")]

use crate::common;
use porter_ids::word::word as word_of;

use common::bus::PrivateBus;
use common::eventually;
use common::fake_accountd::{FakeAccountd, KEY, Sheets};
use porter_capability::{AgentProgram, EnvName};
use porter_client::credential::{ChildKey, ChildKeyError, Delivery};
use porter_client::{Launcher, LauncherError};
use porter_core::GrantId;
use porter_core::audit::CredentialEnd;
use porter_dbus::BusStream;
use porter_dbus::accounts::LauncherFault;
use porter_wire::Refusal;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;

fn program() -> AgentProgram {
    AgentProgram::parse("claude-code").expect("program")
}

fn permit(id: &str) -> GrantId {
    GrantId::parse(id).expect("permit")
}

fn key_env() -> EnvName {
    EnvName::parse("ANTHROPIC_API_KEY").expect("var")
}

struct Rig {
    _bus: PrivateBus,
    daemon: zbus::Connection,
    fake: FakeAccountd,
    launcher: Launcher,
}

async fn rig() -> Rig {
    let bus = PrivateBus::start();
    let daemon = bus.connect().await;
    let fake = FakeAccountd::new(Sheets::Hang);
    fake.serve(&daemon).await;
    let launcher = Launcher::connect(&bus.connect().await)
        .await
        .expect("launcher");
    launcher.register(&[program()]).await.expect("registered");
    Rig {
        _bus: bus,
        daemon,
        fake,
        launcher,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_memfd_credential_sets_a_child_up_by_value_or_by_an_inherited_descriptor() {
    let rig = rig().await;
    let credential = rig
        .launcher
        .issue_credential(&permit("g-always"), &program())
        .await
        .expect("issued");
    assert_eq!(
        rig.fake.seen(|seen| seen.issued.clone()),
        [("g-always".to_owned(), "claude-code".to_owned())],
        "the permit, not a grant, is what is asked by"
    );

    // The key, read without moving the position the descriptor's holders share.
    assert_eq!(credential.read_key().expect("key").expose(), KEY);
    assert_eq!(credential.read_key().expect("again").expose(), KEY);
    let mut shared = std::fs::File::from(
        credential
            .descriptor()
            .as_fd()
            .try_clone_to_owned()
            .expect("dup"),
    );
    let mut text = String::new();
    shared.read_to_string(&mut text).expect("a plain read");
    assert_eq!(text, KEY, "still at the start after read_key");

    // By value: the variable the program declares, holding the key.
    match credential
        .child_key(&key_env(), Delivery::Value)
        .expect("by value")
    {
        ChildKey::Value { var, value } => {
            assert_eq!(var.as_str(), "ANTHROPIC_API_KEY");
            assert_eq!(value.expose(), KEY);
        }
        other => panic!("{other:?}"),
    }
    // By descriptor: `<VAR>_FILE` names /proc/self/fd/<n> and a duplicate is to be placed there.
    match credential
        .child_key(&key_env(), Delivery::File { child_fd: 7 })
        .expect("by descriptor")
    {
        ChildKey::File { var, path, inherit } => {
            assert_eq!(var.as_str(), "ANTHROPIC_API_KEY_FILE");
            assert_eq!(path, "/proc/self/fd/7");
            assert_eq!(inherit.number, 7);
            let file = std::fs::File::from(inherit.fd);
            let mut bytes = vec![0u8; KEY.len()];
            file.read_exact_at(&mut bytes, 0).expect("read");
            assert_eq!(bytes, KEY.as_bytes());
        }
        other => panic!("{other:?}"),
    }

    // Ended by the launcher when the process is over; once.
    rig.launcher
        .revoke_credential(credential.id())
        .await
        .expect("revoked");
    assert_eq!(
        rig.launcher.revoke_credential(credential.id()).await,
        Err(LauncherError::Fault(LauncherFault::UnknownCredential))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn accountd_ending_a_credential_reaches_the_launcher_as_a_revocation() {
    let rig = rig().await;
    let mut revocations = rig.launcher.revocations().await.expect("stream");
    let credential = rig
        .launcher
        .issue_credential(&permit("g-always"), &program())
        .await
        .expect("issued");

    rig.fake
        .revoke_for_launcher(
            &rig.daemon,
            credential.id().as_str(),
            word_of(&CredentialEnd::GrantRevoked),
        )
        .await;

    let told = tokio::time::timeout(
        test_kit::GENEROUS,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut revocations).poll_next(cx)),
    )
    .await
    .expect("in time")
    .expect("open")
    .expect("well formed");
    assert_eq!(&told.id, credential.id());
    assert_eq!(told.reason, CredentialEnd::GrantRevoked);
    // The launcher ends the process and tells accountd.
    rig.launcher
        .revoke_credential(credential.id())
        .await
        .expect("revoked");
    eventually("the credential to be ended", || {
        rig.fake.seen(|seen| seen.revoked.len()) == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_a_typed_error_and_a_key_is_never_part_of_it() {
    let rig = rig().await;
    assert!(matches!(
        rig.launcher
            .issue_credential(&permit("g-once"), &program())
            .await,
        Err(LauncherError::Fault(LauncherFault::OnceGrant))
    ));
    assert!(matches!(
        rig.launcher
            .issue_credential(&permit("g-gone"), &program())
            .await,
        Err(LauncherError::Refused(Refusal::UnknownGrant))
    ));
}

#[test]
fn a_child_key_error_names_what_went_wrong_without_the_key() {
    let text = ChildKeyError::NotText.to_string();
    assert!(!text.contains("sk-"));
}
