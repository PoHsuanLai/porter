use porter_fake_servers::{Daemon, FakeLocalApi, FakePeer, Network};
use porter_tailscale::LocalApi;
use std::ops::Deref;
use test_kit::Scratch;

/// A new empty directory for one test, under the test's temporary directory.
pub fn scratch(name: &str) -> Scratch {
    Scratch::new(&format!("tailscale-{name}"))
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

/// A fake Tailscale and the directory its socket is in; the directory goes after the fake stops.
pub struct Fake {
    api: FakeLocalApi,
    _dir: Scratch,
}

impl Deref for Fake {
    type Target = FakeLocalApi;

    fn deref(&self) -> &FakeLocalApi {
        &self.api
    }
}

/// A fake tailscaled, and a client of it that finds no Tailscale program on this machine.
pub async fn start(name: &str, daemon: Daemon) -> (Fake, LocalApi) {
    let dir = scratch(name);
    let fake = FakeLocalApi::start(&dir, daemon).await.expect("fake");
    let api = LocalApi::new(fake.socket()).with_program_dirs(Vec::new());
    let fake = Fake {
        api: fake,
        _dir: dir,
    };
    (fake, api)
}
