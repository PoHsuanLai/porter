//! A key for a process the launcher spawns, through `porter-client`'s `Launcher`, against a
//! hand-written inferd (`common::fake_inferd`) on a private bus: issue by memfd and by tmpfs
//! file, set a child up with it, end it, and hear inferd end it. What inferd checks before it
//! issues one (the permit, the program, the caller) is in inferd's launcher tests.
#![cfg(feature = "dbus")]

use crate::common;

use common::bus::PrivateBus;
use common::eventually;
use common::fake_inferd::{FakeInferd, KEY, Sheets};
use porter_capability::{AgentProgram, EnvName};
use porter_client::credential::{ChildKey, ChildKeyError, CredentialHandle, Delivery};
use porter_client::{Launcher, LauncherError};
use porter_core::GrantId;
use porter_core::audit::{CredentialEnd, Handoff};
use porter_dbus::{BusStream, LauncherFault};
use porter_wire::Refusal;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::fs::{FileExt, PermissionsExt};

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
    bus: PrivateBus,
    daemon: zbus::Connection,
    fake: FakeInferd,
    launcher: Launcher,
}

async fn rig() -> Rig {
    let bus = PrivateBus::start();
    let daemon = bus.connect().await;
    let fake = FakeInferd::new(bus.scratch().to_path_buf(), Sheets::Hang);
    fake.serve(&daemon).await;
    let launcher = Launcher::connect(&bus.connect().await)
        .await
        .expect("launcher");
    launcher.register(&[program()]).await.expect("registered");
    Rig {
        bus,
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
        .issue_credential(&permit("g-always"), &program(), Handoff::Memfd)
        .await
        .expect("issued");
    assert_eq!(credential.handoff(), Handoff::Memfd);
    assert!(matches!(credential.handle(), CredentialHandle::Memfd(_)));
    assert_eq!(
        rig.fake.seen(|seen| seen.issued.clone()),
        [(
            "g-always".to_owned(),
            "claude-code".to_owned(),
            "memfd".to_owned()
        )],
        "the permit, not a grant, is what is asked by"
    );

    // The key, read without moving the position the descriptor's holders share.
    assert_eq!(credential.read_key().expect("key").expose(), KEY);
    assert_eq!(credential.read_key().expect("again").expose(), KEY);
    if let CredentialHandle::Memfd(fd) = credential.handle() {
        let mut shared = std::fs::File::from(fd.as_fd().try_clone_to_owned().expect("dup"));
        let mut text = String::new();
        shared.read_to_string(&mut text).expect("a plain read");
        assert_eq!(text, KEY, "still at the start after read_key");
    }

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
            let inherit = inherit.expect("a descriptor to pass");
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
async fn a_tmpfs_credential_is_a_private_file_that_ending_it_removes() {
    let rig = rig().await;
    let credential = rig
        .launcher
        .issue_credential(&permit("g-always"), &program(), Handoff::TmpfsFile)
        .await
        .expect("issued");
    let CredentialHandle::TmpfsFile(path) = credential.handle() else {
        panic!("a path");
    };
    assert!(path.starts_with(rig.bus.scratch()));
    assert_eq!(
        std::fs::metadata(path).expect("file").permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(credential.read_key().expect("key").expose(), KEY);
    match credential
        .child_key(&key_env(), Delivery::File { child_fd: 3 })
        .expect("by file")
    {
        ChildKey::File {
            var,
            path: shown,
            inherit,
        } => {
            assert_eq!(var.as_str(), "ANTHROPIC_API_KEY_FILE");
            assert_eq!(shown, path.to_string_lossy());
            assert!(inherit.is_none(), "a path needs no descriptor");
        }
        other => panic!("{other:?}"),
    }
    rig.launcher
        .revoke_credential(credential.id())
        .await
        .expect("revoked");
    assert!(!path.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn inferd_ending_a_credential_reaches_the_launcher_as_a_revocation() {
    let rig = rig().await;
    let mut revocations = rig.launcher.revocations().await.expect("stream");
    let credential = rig
        .launcher
        .issue_credential(&permit("g-always"), &program(), Handoff::TmpfsFile)
        .await
        .expect("issued");

    rig.fake
        .revoke_for_launcher(
            &rig.daemon,
            credential.id().as_str(),
            CredentialEnd::GrantRevoked.word(),
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
    // The launcher ends the process and tells inferd; the file goes with it.
    rig.launcher
        .revoke_credential(credential.id())
        .await
        .expect("revoked");
    eventually("the file to go", || match credential.handle() {
        CredentialHandle::TmpfsFile(path) => !path.exists(),
        _ => false,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_a_typed_error_and_a_key_is_never_part_of_it() {
    let rig = rig().await;
    assert!(matches!(
        rig.launcher
            .issue_credential(&permit("g-once"), &program(), Handoff::Memfd)
            .await,
        Err(LauncherError::Fault(LauncherFault::OnceGrant))
    ));
    assert!(matches!(
        rig.launcher
            .issue_credential(&permit("g-gone"), &program(), Handoff::TmpfsFile)
            .await,
        Err(LauncherError::Refused(Refusal::UnknownGrant))
    ));
}

#[test]
fn a_child_key_error_names_what_went_wrong_without_the_key() {
    let text = ChildKeyError::NotText.to_string();
    assert!(!text.contains("sk-"));
}
