use super::*;
use porter_capability::{Access, Delta, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::{AppId, AppName, DataClass, Isolation};
use porter_fake::{Scripted, ScriptedSheets, fake_service};
use porter_ids::UnixSeconds;
use porter_match::Need;
use porter_match::StorageNeed;
use porter_service_core::audit::AuditEvent;
use porter_wire::ParentWindow;
use porter_wire::{AccountsReply, AccountsRequest};
use test_kit::Scratch;

fn entry(event: AuditEvent) -> AuditEntry {
    AuditEntry::new(UnixSeconds(1_790_000_000), None, None, event)
}

fn lines(path: &PathBuf) -> Vec<AuditEntry> {
    std::fs::read_to_string(path)
        .expect("read")
        .lines()
        .map(|l| serde_json::from_str(l).expect("an entry per line"))
        .collect()
}

#[test]
fn entries_append_one_per_line_and_the_directory_is_made() {
    let scratch = Scratch::new("audit");
    let path = scratch.join("quire").join("accountd").join("audit.jsonl");
    let sink = FileAudit::new(path.clone());
    sink.record(entry(AuditEvent::SignedIn));
    // A second sink on the same file appends; nothing is truncated.
    FileAudit::new(path.clone()).record(entry(AuditEvent::Removed));
    sink.record(entry(AuditEvent::Reauthed));
    assert_eq!(
        lines(&path),
        vec![
            entry(AuditEvent::SignedIn),
            entry(AuditEvent::Removed),
            entry(AuditEvent::Reauthed)
        ]
    );
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(text.ends_with('\n'));
    assert_eq!(text.lines().count(), 3);
}

#[cfg(unix)]
#[test]
fn the_file_is_readable_by_its_owner_alone() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("audit");
    let path = scratch.join("audit.jsonl");
    FileAudit::new(path.clone()).record(entry(AuditEvent::SignedIn));
    let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_file_that_cannot_be_written_does_not_fail_the_caller() {
    let scratch = Scratch::new("audit");
    // The path is a directory: the append fails, and `record` still returns.
    FileAudit::new(scratch.path().to_path_buf()).record(entry(AuditEvent::SignedIn));
}

#[tokio::test]
async fn the_file_a_service_writes_holds_none_of_the_fake_secrets() {
    let scratch = Scratch::new("audit");
    let path = scratch.join("audit.jsonl");
    let sheets = ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let service = fake_service(sheets)
        .await
        .with_audit(FileAudit::new(path.clone()));
    let me = AppId {
        name: AppName::parse("org.quire.Photos").expect("name"),
        isolation: Isolation::Flatpak,
    };
    let need = Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        false,
    ));
    let chosen = service
        .handle(
            &me,
            AccountsRequest::Choose {
                need,
                class: DataClass::Files,
                usage: Usage::Interactive,
                window: ParentWindow::Unparented,
                account: None,
            },
        )
        .await;
    let AccountsReply::Chosen(candidate) = chosen else {
        panic!("not chosen: {chosen:?}");
    };
    let endpoint = candidate.endpoints[0].url.clone();
    service
        .open_authenticated(&me, &candidate.grant, &endpoint)
        .await
        .expect("planned");
    service
        .remove_account(&candidate.account)
        .await
        .expect("removed");

    assert_eq!(lines(&path).len(), 3);
    let text = std::fs::read_to_string(&path).expect("read");
    // The values porter-fake files or mints: passwords, OAuth tokens, issued tokens.
    for secret in [
        "app-pw",
        "access",
        "refresh",
        "fake-signed-in",
        "fake:fake-",
    ] {
        assert!(!text.contains(secret), "{secret} in the audit file");
    }
}
