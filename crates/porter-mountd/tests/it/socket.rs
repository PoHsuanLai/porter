//! The client against a helper listening on a socket path, and the fallback when none listens.

use crate::common::FakeMounts;
use porter_mountd::client::{self, AskError};
use porter_mountd::server::{self, Helper};
use porter_mountd::wire::{self, Access, Code, FolderName};
use std::os::unix::fs::PermissionsExt;
use test_kit::Scratch;

#[test]
fn the_client_reaches_a_helper_at_its_socket_and_gets_its_code() {
    let dir = Scratch::new("mountd-socket");
    let socket = wire::socket_in(&dir.join("run"));
    let listener = server::bind_at(&socket).expect("bind");
    let mode = |path: &std::path::Path| {
        std::fs::metadata(path).expect("meta").permissions().mode() & 0o777
    };
    assert_eq!(mode(&socket), 0o600);
    assert_eq!(mode(socket.parent().expect("dir")), 0o700);
    // A second helper does not take the socket over.
    assert!(server::bind_at(&socket).is_err());
    let helper = Helper::new(dir.join("home/Cloud"), FakeMounts::default());
    let name = FolderName::parse("OneDrive").expect("name");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            // The second bind's look for a helper there, then the two requests.
            for _ in 0..3 {
                let (stream, _) = listener.accept().expect("accept");
                let _ = helper.answer(&stream);
            }
        });
        client::mount(&socket, &name, Access::ReadOnly).expect("the connection");
        match client::mount(&socket, &name, Access::ReadOnly) {
            Err(AskError::Refused(said)) => assert_eq!(said, Code::AlreadyShown),
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn no_helper_at_the_socket_is_told_apart() {
    let dir = Scratch::new("mountd-absent");
    let socket = wire::socket_in(&dir.join("run"));
    let name = FolderName::parse("OneDrive").expect("name");
    assert!(matches!(
        client::mount(&socket, &name, Access::ReadOnly),
        Err(AskError::Absent)
    ));
    assert!(matches!(
        client::unmount(&socket, &name),
        Err(AskError::Absent)
    ));
    // A socket left by a helper that is gone is as good as none, and a new helper binds over it.
    drop(server::bind_at(&socket).expect("bind"));
    assert!(matches!(
        client::mount(&socket, &name, Access::ReadOnly),
        Err(AskError::Absent)
    ));
    server::bind_at(&socket).expect("bound again over the one left");
}
