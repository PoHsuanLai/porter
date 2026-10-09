//! The crate's integration tests: the LocalAPI client against a fake tailscaled on a unix socket
//! in a scratch directory. Nothing here touches the real Tailscale or `/var/run/tailscale`.

mod common;
mod errors;
mod status;
mod watch;
mod whois;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
