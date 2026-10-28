use super::*;
use porter_core::{AppName, Isolation};
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::io::Read;

const KEY: &str = "sk-ant-api03-S3CRET-HANDOFF-KEY";

fn audience() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Agent.claude-code").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn issue(
    credentials: &Credentials,
    owner: &str,
) -> Result<(ProcessCredentialId, OwnedFd), Refusal> {
    let (audience, account, grant, key) = (
        audience(),
        AccountId::parse("anthropic").expect("id"),
        GrantId::parse("g1").expect("id"),
        SecretText::new(KEY),
    );
    credentials.issue(&Issue {
        owner,
        audience: &audience,
        account: &account,
        grant: &grant,
        key: &key,
    })
}

#[test]
fn a_credential_is_a_sealed_descriptor_holding_the_key() {
    let credentials = Credentials::default();
    let (id, fd) = issue(&credentials, ":1.7").expect("issued");
    assert_eq!(
        fcntl_get_seals(&fd).expect("seals"),
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
    );
    let mut text = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut text)
        .expect("read");
    assert_eq!(text, KEY);
    assert!(credentials.take(":1.7", &id, |_| {}).is_some());
}

#[test]
fn only_the_connection_issued_a_credential_can_end_it_and_an_end_is_noted_once() {
    let credentials = Credentials::default();
    let (id, _) = issue(&credentials, ":1.1").expect("issued");
    assert!(
        credentials
            .take(":1.2", &id, |_| panic!("another's"))
            .is_none()
    );
    let noted = std::cell::RefCell::new(Vec::new());
    let seen = credentials.take(":1.1", &id, |held| {
        noted.borrow_mut().push(held.owner.clone());
    });
    assert!(seen.is_some());
    assert_eq!(noted.take(), [":1.1".to_owned()]);
    assert!(
        credentials
            .take(":1.1", &id, |_| panic!("already ended"))
            .is_none()
    );
}

#[test]
fn two_credentials_get_two_ids_and_ending_one_leaves_the_other() {
    let credentials = Credentials::default();
    let (first, _) = issue(&credentials, ":1.1").expect("first");
    let (second, _) = issue(&credentials, ":1.2").expect("second");
    assert_ne!(first, second);
    let noted = std::cell::RefCell::new(Vec::new());
    let ended = credentials.end_where(
        |held| (held.owner == ":1.1").then_some(CredentialEnd::LauncherGone),
        |held, reason| noted.borrow_mut().push((held.owner.clone(), reason)),
    );
    assert_eq!(ended.len(), 1);
    assert_eq!(
        (&ended[0].0, ended[0].2),
        (&first, CredentialEnd::LauncherGone)
    );
    assert_eq!(
        noted.take(),
        [(":1.1".to_owned(), CredentialEnd::LauncherGone)]
    );
    assert!(credentials.take(":1.2", &second, |_| {}).is_some());
}
