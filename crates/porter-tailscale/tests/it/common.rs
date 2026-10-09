use porter_fake_servers::{Daemon, FakeLocalApi, FakePeer, Network};
use porter_tailscale::LocalApi;
use std::path::PathBuf;

/// A new empty directory for one test, under the test's temporary directory.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("porter-tailscale-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

pub const LOGIN_PAGE: &str = "https://login.tailscale.com/a/0123456789abcdef";

/// A small network: this computer (`desk`) and a few others of every kind.
pub fn network() -> Network {
    Network::new(
        "ada@example.org",
        "ada@example.org",
        FakePeer::new("nSELF", "desk", 1, "100.64.0.1"),
    )
    .with_user(2, "bob@example.net")
    .with_peers(vec![
        FakePeer::new("nPI", "pi", 1, "100.64.0.2").with_ssh(),
        FakePeer::new("nOLD", "old-laptop", 1, "100.64.0.3").offline("2026-09-21T14:13:20Z"),
        FakePeer::new("nBUILD", "build-box", 1, "100.64.0.4").tagged("tag:ci"),
        FakePeer::new("nFRIEND", "friends-pc", 2, "100.64.0.5").shared_in(),
    ])
}

/// A fake tailscaled, and a client of it that finds no Tailscale program on this machine.
pub async fn start(name: &str, daemon: Daemon) -> (FakeLocalApi, LocalApi) {
    let dir = scratch(name);
    let fake = FakeLocalApi::start(&dir, daemon).await.expect("fake");
    let api = LocalApi::new(fake.socket()).with_program_dirs(Vec::new());
    (fake, api)
}
