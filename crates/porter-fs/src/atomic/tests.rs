use super::*;
use std::sync::atomic::AtomicU32;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory under the system temp directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "porter-core-atomic-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_write_replaces_the_file_whole_and_leaves_nothing_staged() {
    let scratch = Scratch::new();
    let target = scratch.0.join("a.json");
    AtomicWrite::SHARED.write(&target, b"one").expect("first");
    AtomicWrite::SHARED.write(&target, b"two!").expect("second");
    assert_eq!(std::fs::read(&target).expect("read"), b"two!");
    assert_eq!(names(&scratch.0), ["a.json"]);
}

#[test]
fn a_write_reaches_the_disk_before_it_is_published_and_the_directory_after() {
    let scratch = Scratch::new();
    let before = sync_count::so_far();
    AtomicWrite::SHARED
        .in_existing_dir()
        .write(&scratch.0.join("a.ics"), b"x")
        .expect("written");
    let after = sync_count::so_far();
    assert_eq!(after.0 - before.0, 1, "the file was synced");
    #[cfg(unix)]
    assert_eq!(after.1 - before.1, 1, "the directory was synced");
}

#[test]
fn a_write_in_a_directory_that_is_gone_fails_and_does_not_make_it() {
    let scratch = Scratch::new();
    let gone = scratch.0.join("gone");
    let error = AtomicWrite::SHARED
        .in_existing_dir()
        .write(&gone.join("displayname"), b"x")
        .expect_err("no directory");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(!gone.exists(), "the directory was not brought back");
    AtomicWrite::PRIVATE
        .write(&gone.join("deeper/file"), b"x")
        .expect("the other kind makes it");
    assert!(gone.join("deeper/file").exists());
}

#[cfg(unix)]
#[test]
fn private_means_the_owner_alone() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let target = scratch.0.join("keys");
    AtomicWrite::PRIVATE.write(&target, b"x").expect("written");
    let mode = std::fs::metadata(&target)
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_copy_publishes_the_source_whole() {
    let scratch = Scratch::new();
    let source = scratch.0.join("source");
    std::fs::write(&source, b"photo bytes").expect("source");
    let target = scratch.0.join("aa/target");
    AtomicWrite::SHARED.copy(&source, &target).expect("copied");
    assert_eq!(std::fs::read(&target).expect("read"), b"photo bytes");
    assert_eq!(names(&scratch.0.join("aa")), ["target"]);
    let missing = AtomicWrite::SHARED.copy(&scratch.0.join("nope"), &scratch.0.join("t2"));
    assert!(missing.is_err());
    assert_eq!(
        names(&scratch.0),
        ["aa", "source"],
        "a failed copy leaves nothing"
    );
}

#[test]
fn staging_names_are_dot_files_ending_in_tmp_and_never_repeat() {
    let target = Path::new("/some/dir/registry.json");
    let all: std::collections::HashSet<PathBuf> = (0..64).map(|_| staging_beside(target)).collect();
    assert_eq!(all.len(), 64);
    for name in &all {
        assert_eq!(name.parent(), Some(Path::new("/some/dir")));
        let name = name.file_name().expect("name").to_string_lossy();
        assert!(
            name.starts_with(".tmp-") && name.ends_with(".tmp"),
            "{name}"
        );
    }
}

#[test]
fn a_target_with_no_directory_part_is_written_in_the_current_one() {
    assert_eq!(directory_of(Path::new("file")), Path::new("."));
    assert_eq!(directory_of(Path::new("/x/file")), Path::new("/x"));
}

#[test]
fn an_atomic_file_keeps_the_one_it_replaces_as_the_backup() {
    let scratch = Scratch::new();
    let file = AtomicFile::new(scratch.0.join("state"), "doc.json");
    assert_eq!(file.read().expect("read"), None);
    file.write("one").expect("first");
    assert!(
        !file.backup_path().exists(),
        "nothing to keep on the first save"
    );
    file.write("two").expect("second");
    assert_eq!(file.read().expect("read"), Some(b"two".to_vec()));
    assert_eq!(std::fs::read(file.backup_path()).expect("bak"), b"one");
    file.write("three").expect("third");
    assert_eq!(std::fs::read(file.backup_path()).expect("bak"), b"two");
    assert_eq!(
        names(&scratch.0.join("state")),
        ["doc.json", "doc.json.bak"]
    );
}

#[test]
fn a_staged_file_is_not_the_file_until_it_is_committed() {
    let scratch = Scratch::new();
    let file = AtomicFile::new(scratch.0.clone(), "doc.json");
    file.write("old").expect("saved");
    let stale = file.stage("new").expect("staged");
    assert_eq!(file.read().expect("read"), Some(b"old".to_vec()));
    file.write("newer").expect("saved");
    assert!(stale.exists(), "another save leaves a staged file alone");
    assert_ne!(stale, file.staging());
}
