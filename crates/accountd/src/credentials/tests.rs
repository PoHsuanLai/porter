use super::*;
use porter_core::{AppName, Isolation};
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};

const KEY: &str = "sk-ant-api03-S3CRET-HANDOFF-KEY";

/// A scratch directory of its own, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "accountd-credentials-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn audience() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Agent.claude-code").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn issue(
    credentials: &Credentials,
    owner: &str,
    handoff: Handoff,
) -> Result<(ProcessCredentialId, Handle), Refusal> {
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
        handoff,
    })
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn a_tmpfs_credential_is_a_0600_file_in_a_0700_directory_of_its_own_and_ending_it_unlinks_both() {
    let scratch = Scratch::new();
    let credentials = Credentials::new(Some(&scratch.0));
    let (id, handle) = issue(&credentials, ":1.7", Handoff::TmpfsFile).expect("issued");
    let Handle::Path(path) = handle else {
        panic!("a tmpfs credential is a path");
    };
    let root = scratch.0.join("porter").join("agent");
    assert_eq!(path, root.join(id.as_str()).join("key"));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), KEY);
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().expect("dir")), 0o700);
    assert_eq!(mode(&root), 0o700);

    // Only the connection it was issued to may end it.
    assert!(credentials.take(":1.8", &id, |_| {}).is_none());
    assert!(path.exists());
    let held = credentials.take(":1.7", &id, |_| {}).expect("ended");
    assert_eq!(held.owner, ":1.7");
    assert!(!path.exists());
    assert!(!path.parent().expect("dir").exists());
    assert!(root.exists(), "the agent directory itself stays");
    assert!(credentials.take(":1.7", &id, |_| {}).is_none(), "once");
}

#[test]
fn the_end_is_noted_while_the_file_is_still_there() {
    // The audit line comes first: a reader who sees the file gone finds the line that says why
    // (rel-13: a launcher's credentials were unlinked, then audited, and a test that looked in
    // between found no line).
    let scratch = Scratch::new();
    let credentials = Credentials::new(Some(&scratch.0));
    let (gone, one) = issue(&credentials, ":1.1", Handoff::TmpfsFile).expect("first");
    let (taken, two) = issue(&credentials, ":1.2", Handoff::TmpfsFile).expect("second");
    let (Handle::Path(one), Handle::Path(two)) = (one, two) else {
        panic!("paths");
    };
    let noted = std::cell::RefCell::new(Vec::new());
    credentials.end_where(
        |held| (held.owner == ":1.1").then_some(CredentialEnd::LauncherGone),
        |held, reason| {
            noted
                .borrow_mut()
                .push((held.owner.clone(), reason, one.exists()))
        },
    );
    assert_eq!(
        noted.take(),
        [(":1.1".to_owned(), CredentialEnd::LauncherGone, true)]
    );
    assert!(!one.exists());
    let seen = credentials.take(":1.2", &taken, |held| {
        noted.borrow_mut().push((
            held.owner.clone(),
            CredentialEnd::ProcessExited,
            two.exists(),
        ));
    });
    assert!(seen.is_some());
    assert_eq!(
        noted.take(),
        [(":1.2".to_owned(), CredentialEnd::ProcessExited, true)]
    );
    assert!(!two.exists());
    assert!(
        credentials
            .take(":1.1", &gone, |_| panic!("not held"))
            .is_none()
    );
}

#[test]
fn two_credentials_get_two_files_and_ending_one_leaves_the_other() {
    let scratch = Scratch::new();
    let credentials = Credentials::new(Some(&scratch.0));
    let (first, one) = issue(&credentials, ":1.1", Handoff::TmpfsFile).expect("first");
    let (second, two) = issue(&credentials, ":1.2", Handoff::TmpfsFile).expect("second");
    assert_ne!(first, second);
    let (Handle::Path(one), Handle::Path(two)) = (one, two) else {
        panic!("paths");
    };
    let ended = credentials.end_where(
        |held| (held.owner == ":1.1").then_some(CredentialEnd::LauncherGone),
        |_, _| {},
    );
    assert_eq!(ended.len(), 1);
    assert_eq!(
        (&ended[0].0, ended[0].2),
        (&first, CredentialEnd::LauncherGone)
    );
    assert!(!one.exists());
    assert!(two.exists());
}

#[test]
fn a_memfd_credential_is_a_sealed_descriptor_and_leaves_no_file() {
    let scratch = Scratch::new();
    let credentials = Credentials::new(Some(&scratch.0));
    let (id, handle) = issue(&credentials, ":1.7", Handoff::Memfd).expect("issued");
    let Handle::Fd(fd) = handle else {
        panic!("a memfd credential is a descriptor");
    };
    assert_eq!(
        fcntl_get_seals(&fd).expect("seals"),
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
    );
    let mut text = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut text)
        .expect("read");
    assert_eq!(text, KEY);
    assert!(
        !scratch.0.join("porter").exists(),
        "a memfd touches no directory"
    );
    assert!(credentials.take(":1.7", &id, |_| {}).is_some());
}

#[test]
fn a_tmpfs_credential_needs_a_runtime_directory() {
    let credentials = Credentials::new(None);
    assert!(matches!(
        issue(&credentials, ":1.7", Handoff::TmpfsFile),
        Err(Refusal::Unavailable)
    ));
    // The other way does not.
    assert!(issue(&credentials, ":1.7", Handoff::Memfd).is_ok());
}

#[test]
fn what_an_earlier_accountd_left_is_cleared_at_start() {
    let scratch = Scratch::new();
    let stale = scratch.0.join("porter").join("agent").join("cred-1");
    std::fs::create_dir_all(&stale).expect("stale dir");
    std::fs::write(stale.join("key"), KEY).expect("stale key");
    let credentials = Credentials::new(Some(&scratch.0));
    assert!(!stale.exists());
    // The first id of this run does not meet the old directory.
    let (_, handle) = issue(&credentials, ":1.7", Handoff::TmpfsFile).expect("issued");
    assert!(matches!(handle, Handle::Path(path) if path.exists()));
}

#[test]
fn a_directory_that_is_there_already_is_refused_rather_than_written_into() {
    let scratch = Scratch::new();
    let credentials = Credentials::new(Some(&scratch.0));
    let planted = scratch.0.join("porter").join("agent").join("cred-1");
    std::fs::create_dir_all(&planted).expect("planted");
    assert!(matches!(
        issue(&credentials, ":1.7", Handoff::TmpfsFile),
        Err(Refusal::Unavailable)
    ));
    assert!(std::fs::read_dir(&planted).expect("dir").next().is_none());
}
