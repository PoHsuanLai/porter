//! The accounts-only socket carrier over latchkey: no inference, no porter-infer (this file builds
//! with `--no-default-features --features socket`). The agent end is latchkey's own
//! (`Agent::listen`, the single-instance lock and the address rule), in a scratch runtime
//! directory; no real agent and no real session is touched.
#![cfg(all(unix, feature = "socket"))]

use latchkey::{Agent, Environment, Stream, here};
use porter_client::{
    Accounts, ClientEnv, ClientError, LinkChoice, SocketAgent, SocketTransport, StartAgent,
    TransportError,
};
use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_core::{AccountsReply, AccountsRequest};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const NAME: &str = "pt";

/// A runtime directory of this test's own (short: the socket path has 108 bytes).
fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "sa-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// latchkey's agent under `dir`, as the agent end names it.
fn agent_in(dir: &Path) -> Agent {
    let env = Environment {
        runtime_dir: Some(dir.as_os_str()),
        tmpdir: Some(dir.as_os_str()),
        ..Environment::default()
    };
    Agent::in_environment(NAME, here(), &env).expect("an address")
}

/// Answers `ListGrants` with no grants; false when the client sent nothing (a probe).
fn answer(client: &mut Stream) -> bool {
    let mut head = [0u8; 4];
    if client.read_exact(&mut head).is_err() {
        return false;
    }
    let mut body = vec![0u8; u32::from_be_bytes(head) as usize];
    if client.read_exact(&mut body).is_err() {
        return false;
    }
    let bytes: Vec<u8> = head.into_iter().chain(body).collect();
    let Ok(FrameRead::Complete(envelope, _)) = decode_frame::<AccountsRequest>(&bytes) else {
        return false;
    };
    assert_eq!(envelope.body, AccountsRequest::ListGrants);
    let reply = encode_frame(&AccountsReply::Grants(vec![])).expect("frame");
    client.write_all(&reply).is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_finds_the_agent_latchkey_listens_for() {
    let dir = scratch("door");
    let door = agent_in(&dir).listen().expect("listen");
    let served = std::thread::spawn(move || {
        while let Ok(mut client) = door.accept() {
            if answer(&mut client) {
                return;
            }
        }
    });

    let accounts = Accounts::over(SocketTransport::at(
        SocketAgent::named(NAME).in_runtime_dir(dir),
    ));
    assert_eq!(accounts.grants().await.expect("grants"), vec![]);
    served.join().expect("the agent thread");
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_home_is_unreachable_and_nothing_is_started_by_default() {
    let dir = scratch("none");
    let agent = SocketAgent::named(NAME).in_runtime_dir(dir.clone());
    assert_eq!(agent.start, StartAgent::Never);
    let accounts = Accounts::over(SocketTransport::at(agent.clone()));
    assert_eq!(
        accounts.grants().await,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
    let env = ClientEnv {
        links: vec![LinkChoice::Socket(agent)],
    };
    assert!(matches!(
        Accounts::connect(&env).await,
        Err(ClientError::Transport(TransportError::Unreachable))
    ));
    // A client never creates the agent's directory or its socket: that is the agent's.
    assert_eq!(std::fs::read_dir(&dir).expect("dir").count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_that_cannot_be_a_directory_is_malformed_not_unreachable() {
    let dir = scratch("name");
    let accounts = Accounts::over(SocketTransport::at(
        SocketAgent::named("no/slashes").in_runtime_dir(dir),
    ));
    assert!(matches!(
        accounts.grants().await,
        Err(ClientError::Transport(TransportError::Malformed(_)))
    ));
}

/// The agent a spawning client starts: this same test binary, run by libtest with `--ignored` and
/// the runtime directory as a trailing filter (libtest ignores a filter that names no test).
/// Serves clients until it has answered one call, or gives up after twenty seconds.
#[test]
#[ignore = "the agent the spawn test starts; run only as that child"]
fn child_agent() {
    let Some(dir) = std::env::args().next_back().map(PathBuf::from) else {
        return;
    };
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(20));
        std::process::exit(0);
    });
    let door = agent_in(&dir).listen().expect("listen");
    while let Ok(mut client) = door.accept() {
        if answer(&mut client) {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spawning_client_starts_the_agent_and_then_talks_to_it() {
    let dir = scratch("spawn");
    let agent = SocketAgent::named(NAME)
        .in_runtime_dir(dir.clone())
        .starting(StartAgent::Spawn {
            args: vec![
                "--ignored".to_owned(),
                "--exact".to_owned(),
                "child_agent".to_owned(),
                "--nocapture".to_owned(),
                dir.display().to_string(),
            ],
            wait: Duration::from_secs(15),
        });
    let env = ClientEnv {
        links: vec![LinkChoice::Socket(agent)],
    };
    let accounts = Accounts::connect(&env)
        .await
        .expect("the client starts the agent and reaches it");
    assert_eq!(accounts.grants().await.expect("grants"), vec![]);
}
