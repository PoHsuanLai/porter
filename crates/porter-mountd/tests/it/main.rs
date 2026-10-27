//! porter-mountd's tests: requests over real sockets, with a fake mount step (the jail has no
//! /dev/fuse and no fusermount3).

mod common;
mod dist;
mod helper;
mod socket;

#[test]
fn every_test_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
