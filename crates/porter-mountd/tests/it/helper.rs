//! The helper's checks and the fd it hands back, over a socket pair.

use crate::common::FakeMounts;
use porter_mountd::client;
use porter_mountd::server::{At, Helper, Refusal};
use porter_mountd::wire::{self, Access, Code, FolderName, Reply, Request};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use test_kit::Scratch;

fn name(text: &str) -> FolderName {
    FolderName::parse(text).expect("name")
}

fn mount(text: &str) -> Request {
    Request::Mount {
        name: name(text),
        access: Access::ReadOnly,
    }
}

/// `~/Cloud` in a scratch home, and its real path.
fn cloud(dir: &Scratch) -> (PathBuf, PathBuf) {
    let cloud = dir.join("home/Cloud");
    std::fs::create_dir_all(&cloud).expect("cloud");
    let real = cloud.canonicalize().expect("real");
    (cloud, real)
}

/// Sends `bytes` as a request to a helper answering on the other end of a socket pair.
fn exchange(
    helper: &Helper<FakeMounts>,
    bytes: &[u8],
) -> (Result<(), Refusal>, Reply, Option<std::os::fd::OwnedFd>) {
    let (ours, theirs) = UnixStream::pair().expect("pair");
    std::thread::scope(|scope| {
        // The helper's end closes once answered, as the server's does, ending the reply.
        let answered = scope.spawn(move || helper.answer(&theirs));
        wire::send(&ours, bytes, None).expect("send");
        ours.shutdown(std::net::Shutdown::Write).expect("shutdown");
        let (reply, fd) = wire::receive(&ours).expect("receive");
        let outcome = answered.join().expect("helper");
        (outcome, Reply::decode(&reply).expect("reply"), fd)
    })
}

#[test]
fn a_mount_hands_back_the_fd_the_mount_step_made_end_to_end() {
    let dir = Scratch::new("mountd-round-trip");
    let (cloud, real) = cloud(&dir);
    let helper = &Helper::new(cloud, FakeMounts::default());
    let (ours, theirs) = UnixStream::pair().expect("pair");
    let fd = std::thread::scope(|scope| {
        // The helper's end closes once answered, as the server's does, ending the reply.
        let answered = scope.spawn(move || helper.answer(&theirs));
        let (reply, fd) = client::ask(&ours, &mount("Google Drive")).expect("ask");
        answered.join().expect("helper").expect("answered");
        assert_eq!(reply, Reply::Done);
        fd.expect("the connection came with the answer")
    });
    // The fd received is the pipe the mount step made: what is written at its end is read here.
    let target = real.join("Google Drive");
    helper
        .mounts()
        .writers
        .lock()
        .expect("writers")
        .get_mut(&target)
        .expect("mounted at ~/Cloud/Google Drive")
        .write_all(b"porter")
        .expect("write");
    helper.mounts().writers.lock().expect("writers").clear();
    let mut got = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut got)
        .expect("read");
    assert_eq!(got, "porter");
    // The folder was made, private to the person.
    let mode = std::fs::metadata(&target)
        .expect("folder")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn a_writable_mount_is_asked_with_its_access() {
    let dir = Scratch::new("mountd-rw");
    let (cloud, real) = cloud(&dir);
    let helper = Helper::new(cloud, FakeMounts::default());
    let request = Request::Mount {
        name: name("OneDrive"),
        access: Access::ReadWrite,
    };
    let (outcome, reply, fd) = exchange(&helper, &request.encode());
    outcome.expect("answered");
    assert_eq!(reply, Reply::Done);
    assert!(fd.is_some());
    assert_eq!(
        helper
            .mounts()
            .access
            .lock()
            .expect("access")
            .get(&real.join("OneDrive")),
        Some(&Access::ReadWrite)
    );
}

#[test]
fn a_refused_name_is_answered_with_a_code_and_nothing_is_touched() {
    let dir = Scratch::new("mountd-names");
    let (cloud, real) = cloud(&dir);
    let helper = Helper::new(cloud, FakeMounts::default());
    let long = "x".repeat(256);
    for bad in [
        "..",
        "../Documents",
        "a/b",
        ".hidden",
        "",
        long.as_str(),
        "nul\u{0}x",
    ] {
        let bytes = [b"1\0mount\0", bad.as_bytes(), b"\0ro\0"].concat();
        let (outcome, reply, fd) = exchange(&helper, &bytes);
        assert!(
            matches!(outcome, Err(Refusal::Message(_))),
            "{bad:?}: {outcome:?}"
        );
        assert_eq!(reply, Reply::Refused(Code::NotHere));
        assert!(fd.is_none());
    }
    assert!(helper.mounts().writers.lock().expect("writers").is_empty());
    assert_eq!(std::fs::read_dir(&real).expect("cloud").count(), 0);
}

#[test]
fn a_folder_that_leads_out_of_cloud_is_refused() {
    let dir = Scratch::new("mountd-outside");
    let (cloud, real) = cloud(&dir);
    let elsewhere = dir.join("Documents");
    std::fs::create_dir_all(&elsewhere).expect("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, real.join("OneDrive")).expect("link");
    std::fs::write(real.join("Google Drive"), b"a file, not a folder").expect("file");
    let helper = Helper::new(cloud, FakeMounts::default());
    for folder in ["OneDrive", "Google Drive"] {
        let refused = helper.handle(&mount(folder)).expect_err(folder);
        assert!(matches!(refused, Refusal::NotInCloud(_)), "{refused:?}");
        assert_eq!(refused.code(), Code::NotHere);
    }
    assert!(helper.mounts().writers.lock().expect("writers").is_empty());
}

#[test]
fn a_folder_with_files_of_its_own_is_refused() {
    let dir = Scratch::new("mountd-not-empty");
    let (cloud, real) = cloud(&dir);
    std::fs::create_dir_all(real.join("OneDrive")).expect("folder");
    std::fs::write(real.join("OneDrive/notes.txt"), b"mine").expect("file");
    let helper = Helper::new(cloud, FakeMounts::default());
    let refused = helper.handle(&mount("OneDrive")).expect_err("not empty");
    assert!(matches!(refused, Refusal::NotEmpty(_)), "{refused:?}");
    assert_eq!(refused.code(), Code::FolderInUse);
    assert!(helper.mounts().writers.lock().expect("writers").is_empty());
    // An empty folder is used as it is.
    std::fs::remove_file(real.join("OneDrive/notes.txt")).expect("rm");
    helper.handle(&mount("OneDrive")).expect("mounted");
}

#[test]
fn one_live_mount_per_name_and_a_stale_one_is_cleared_first() {
    let dir = Scratch::new("mountd-one-per-name");
    let (cloud, real) = cloud(&dir);
    let helper = Helper::new(cloud.clone(), FakeMounts::default());
    helper.handle(&mount("OneDrive")).expect("first");
    let again = helper.handle(&mount("OneDrive")).expect_err("second");
    assert!(matches!(again, Refusal::AlreadyShown(_)), "{again:?}");
    assert_eq!(again.code(), Code::AlreadyShown);
    // Another name is its own.
    helper.handle(&mount("Google Drive")).expect("another name");

    let target = real.join("OneDrive");
    let stale = Helper::new(
        cloud.clone(),
        FakeMounts::with(target.clone(), At::Ours { live: false }),
    );
    stale
        .handle(&mount("OneDrive"))
        .expect("cleared, then mounted");
    assert_eq!(
        *stale.mounts().unmounted.lock().expect("unmounted"),
        vec![target.clone()]
    );

    let other = Helper::new(
        cloud,
        FakeMounts::with(target, At::Other("fuse.sshfs".into())),
    );
    let refused = other
        .handle(&mount("OneDrive"))
        .expect_err("another file system");
    assert!(matches!(refused, Refusal::OtherMount(..)), "{refused:?}");
    assert!(
        other
            .mounts()
            .unmounted
            .lock()
            .expect("unmounted")
            .is_empty()
    );
}

#[test]
fn a_caller_of_another_uid_is_refused() {
    let dir = Scratch::new("mountd-peer");
    let (cloud, _) = cloud(&dir);
    let me = rustix::process::geteuid();
    let someone_else = rustix::process::Uid::from_raw(me.as_raw().wrapping_add(1));
    let helper = Helper::for_owner(cloud, FakeMounts::default(), someone_else);
    let (outcome, reply, fd) = exchange(&helper, &mount("OneDrive").encode());
    assert!(
        matches!(outcome, Err(Refusal::Peer(uid)) if uid == me.as_raw()),
        "{outcome:?}"
    );
    assert_eq!(reply, Reply::Refused(Code::NotHere));
    assert!(fd.is_none());
    assert!(helper.mounts().writers.lock().expect("writers").is_empty());
}

#[test]
fn a_failed_mount_says_the_drive_cannot_be_shown() {
    let dir = Scratch::new("mountd-failed");
    let (cloud, _) = cloud(&dir);
    let helper = Helper::new(
        cloud,
        FakeMounts {
            fail: true,
            ..FakeMounts::default()
        },
    );
    let (outcome, reply, fd) = exchange(&helper, &mount("OneDrive").encode());
    assert!(matches!(outcome, Err(Refusal::Mount(..))), "{outcome:?}");
    assert_eq!(reply, Reply::Refused(Code::NotHere));
    assert!(fd.is_none());
}

#[test]
fn an_unmount_takes_away_only_a_drives_folder() {
    let dir = Scratch::new("mountd-unmount");
    let (cloud, real) = cloud(&dir);
    let helper = Helper::new(cloud.clone(), FakeMounts::default());
    let unmount = |text: &str| Request::Unmount { name: name(text) };
    // Nothing there: done, nothing run.
    assert!(
        helper
            .handle(&unmount("OneDrive"))
            .expect("nothing")
            .is_none()
    );
    assert!(
        helper
            .mounts()
            .unmounted
            .lock()
            .expect("unmounted")
            .is_empty()
    );
    helper.handle(&mount("OneDrive")).expect("mounted");
    helper.handle(&unmount("OneDrive")).expect("unmounted");
    assert_eq!(
        *helper.mounts().unmounted.lock().expect("unmounted"),
        vec![real.join("OneDrive")]
    );
    let other = Helper::new(
        cloud,
        FakeMounts::with(real.join("OneDrive"), At::Other("fuse.sshfs".into())),
    );
    let refused = other.handle(&unmount("OneDrive")).expect_err("not ours");
    assert!(matches!(refused, Refusal::OtherMount(..)), "{refused:?}");
    assert!(
        other
            .mounts()
            .unmounted
            .lock()
            .expect("unmounted")
            .is_empty()
    );
}

#[test]
fn a_missing_cloud_folder_is_made() {
    let dir = Scratch::new("mountd-no-cloud");
    let cloud = dir.join("home/Cloud");
    let helper = Helper::new(cloud.clone(), FakeMounts::default());
    helper.handle(&mount("OneDrive")).expect("mounted");
    assert!(cloud.join("OneDrive").is_dir());
}
