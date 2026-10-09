//! The crate's integration tests: the listeners, the dial, the relay and the hello client
//! against a fake Tailscale on a unix socket in a scratch directory, with every address a
//! loopback address of the test's own. Nothing here touches the real Tailscale, the real
//! network or `/var/run/tailscale`.

mod common;
mod dial;
mod greet;
mod lend;
mod observe;
mod relay;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
