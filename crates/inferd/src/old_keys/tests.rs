use super::*;
use std::os::unix::fs::symlink;
use test_kit::Scratch;

const REGISTRY: &str = "{\"vocab\":13,\"accounts\":[],\"grants\":[],\"toggles\":[]}";

struct Rig {
    scratch: Scratch,
}

impl Rig {
    fn new() -> Self {
        Self {
            scratch: Scratch::new("old-keys"),
        }
    }

    fn registry_dir(&self) -> std::path::PathBuf {
        self.scratch.join("inferd/keys")
    }

    fn audit(&self) -> std::path::PathBuf {
        self.scratch.join("inferd/keys-audit.jsonl")
    }

    fn marker(&self) -> std::path::PathBuf {
        self.scratch.join("accountd/ai-import.json")
    }

    /// The old store as inferd left it.
    fn seed(&self) {
        std::fs::create_dir_all(self.registry_dir()).expect("dir");
        std::fs::write(self.registry_dir().join("registry.json"), REGISTRY).expect("registry");
        std::fs::write(self.registry_dir().join("registry.json.bak"), REGISTRY).expect("bak");
        std::fs::write(self.audit(), "{\"at\":1}\n").expect("audit");
    }

    fn record(&self, of: &str) {
        ImportMarker::of(of.as_bytes())
            .write(&self.marker())
            .expect("marker");
    }

    fn retire(&self) -> Retired {
        retire_at(&self.registry_dir(), &self.audit(), &self.marker())
    }
}

#[test]
fn a_copy_that_covers_the_registry_removes_the_old_files() {
    let rig = Rig::new();
    rig.seed();
    rig.record(REGISTRY);
    assert_eq!(rig.retire(), Retired::Removed);
    assert!(!rig.registry_dir().exists());
    assert!(!rig.audit().exists());
    // Again: nothing is there.
    assert_eq!(rig.retire(), Retired::NothingThere);
}

#[test]
fn no_record_an_unreadable_one_and_a_registry_that_changed_since_the_copy_keep_the_files() {
    let rig = Rig::new();
    rig.seed();
    assert_eq!(rig.retire(), Retired::Kept, "no record");
    std::fs::create_dir_all(rig.marker().parent().expect("dir")).expect("dir");
    std::fs::write(rig.marker(), "not a record").expect("marker");
    assert_eq!(rig.retire(), Retired::Kept, "an unreadable record");
    rig.record("{\"vocab\":12}");
    assert_eq!(rig.retire(), Retired::Kept, "a record of another registry");
    assert!(rig.registry_dir().join("registry.json").exists());
    assert!(rig.audit().exists());
}

#[test]
fn a_directory_that_is_a_link_is_not_followed_or_removed() {
    let rig = Rig::new();
    let elsewhere = rig.scratch.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("dir");
    std::fs::write(elsewhere.join("registry.json"), REGISTRY).expect("registry");
    std::fs::create_dir_all(rig.registry_dir().parent().expect("dir")).expect("dir");
    symlink(&elsewhere, rig.registry_dir()).expect("link");
    rig.record(REGISTRY);
    assert_eq!(rig.retire(), Retired::Kept);
    assert!(elsewhere.join("registry.json").exists());
}

#[test]
fn only_plain_files_go_and_a_link_in_the_directory_is_left() {
    let rig = Rig::new();
    rig.seed();
    rig.record(REGISTRY);
    let target = rig.scratch.join("precious");
    std::fs::write(&target, "x").expect("file");
    symlink(&target, rig.registry_dir().join("link")).expect("link");
    assert_eq!(rig.retire(), Retired::Removed);
    assert!(target.exists(), "what the link names is untouched");
    assert!(
        std::fs::symlink_metadata(rig.registry_dir().join("link")).is_ok(),
        "the link stays, and so does its directory"
    );
}
