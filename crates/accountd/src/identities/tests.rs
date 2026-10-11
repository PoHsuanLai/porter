use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory under the system temp directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "accountd-identities-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn store(&self) -> UsesStore {
        UsesStore::File(self.0.join("porter"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn identity(id: &str) -> IdentityId {
    IdentityId::parse(id).expect("identity")
}

fn app(name: &str) -> AppName {
    AppName::parse(name).expect("app")
}

#[test]
fn a_pick_is_spent_once_and_only_by_the_identity_and_connection_it_names() {
    let picks = Picks::default();
    picks.set(":1.5", identity("ada.example"));
    // Another connection, or another identity, spends nothing and leaves the pick.
    assert!(!picks.take(":1.6", &identity("ada.example")));
    assert!(!picks.take(":1.5", &identity("bob.example")));
    assert!(picks.take(":1.5", &identity("ada.example")));
    assert!(!picks.take(":1.5", &identity("ada.example")), "once");
    // Offering again, or leaving the bus, drops it.
    picks.set(":1.5", identity("ada.example"));
    picks.clear(":1.5");
    assert!(!picks.take(":1.5", &identity("ada.example")));
}

#[tokio::test]
async fn uses_are_kept_once_each_survive_a_restart_and_go_with_the_identity() {
    let scratch = Scratch::new();
    let (mut book, said) = UseBook::open(&scratch.store()).await;
    assert_eq!(said, None);
    assert!(book.apps(&identity("ada.example")).is_empty());
    for _ in 0..2 {
        book.record(identity("ada.example"), app("org.quire.Mail"))
            .await
            .expect("record");
    }
    book.record(identity("ada.example"), app("org.quire.Calendar"))
        .await
        .expect("record");
    let both = [app("org.quire.Calendar"), app("org.quire.Mail")];
    assert_eq!(book.apps(&identity("ada.example")), both);

    let (mut again, said) = UseBook::open(&scratch.store()).await;
    assert_eq!(said, None);
    assert_eq!(again.apps(&identity("ada.example")), both);
    again
        .forget(&identity("ada.example"))
        .await
        .expect("forget");
    again
        .forget(&identity("never.there"))
        .await
        .expect("nothing");
    let (last, _) = UseBook::open(&scratch.store()).await;
    assert!(last.apps(&identity("ada.example")).is_empty());
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_left_as_it_is_and_no_use_is_recorded() {
    let scratch = Scratch::new();
    let dir = scratch.path().join("porter");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join(FILE), "{not json").expect("file");
    let (mut book, said) = UseBook::open(&scratch.store()).await;
    assert!(said.is_some_and(|said| said.contains(FILE)));
    let refused = book
        .record(identity("ada.example"), app("org.quire.Mail"))
        .await;
    assert_eq!(refused, Err(UseFault::Unsaved));
    assert_eq!(
        std::fs::read_to_string(dir.join(FILE)).expect("file"),
        "{not json"
    );
}
