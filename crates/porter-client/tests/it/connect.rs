//! `Accounts::connect`: the first reachable link wins. The session bus is found through the
//! environment, so the D-Bus case runs the connecting half in a child process whose
//! environment names a private bus; this process never edits its own. The child is this test
//! executable run again, by test path (`connect::child_finds_no_bus`).

use crate::common;

use common::bus::PrivateBus;
use common::inferd::{Behaviour, FakeInferd};
use porter_client::{Accounts, ClientEnv, ClientError, LinkChoice, SocketAgent, TransportError};
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{AccountId, DataClass, ModelId, Need, Tier, Tokens};
use porter_fake::{Script, ScriptStep};
use porter_infer::{
    ChatControl, ChatReply, ChatRequest, InferEvent, InferReply, InferRequest, RequestKind,
    ServedBy, StopReason, TokenUsage, ToolParallelism,
};
use std::path::PathBuf;

const CHILD_MARKER: &str = "PORTER_CLIENT_CONNECT_CHILD";
const CHILD_TEST: &str = "connect::child_connects_over_the_session_bus_it_is_given";

fn need() -> Need {
    Need::Llm(LlmNeed::new(Default::default(), Tokens(1)))
}

fn reply() -> InferReply {
    InferReply::Chat(ChatReply::new(
        "connected".into(),
        StopReason::EndTurn,
        TokenUsage {
            input: Tokens(1),
            output: Tokens(1),
            cached: Tokens(0),
        },
        ServedBy {
            account: AccountId::parse("local").expect("id"),
            model: ModelId::parse("echo").expect("id"),
            locality: porter_core::Locality::OnDevice,
        },
    ))
}

fn request() -> InferRequest {
    InferRequest::Chat(
        ChatRequest::new(vec![], Tier::Fast, DataClass::Public, Usage::Interactive)
            .with_control(ChatControl::new().with_tool_calls(ToolParallelism::Many)),
    )
}

fn socket_link() -> LinkChoice {
    LinkChoice::Socket(SocketAgent::porter().in_runtime_dir(PathBuf::from("/nonexistent/runtime")))
}

#[tokio::test]
async fn no_links_is_unreachable() {
    let got = Accounts::connect(&ClientEnv { links: vec![] })
        .await
        .map(|_| ());
    assert_eq!(
        got,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
}

#[tokio::test]
async fn a_socket_link_alone_is_unreachable_until_its_carrier_is_built() {
    let env = ClientEnv {
        links: vec![socket_link()],
    };
    let got = Accounts::connect(&env).await.map(|_| ());
    assert_eq!(
        got,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
}

/// The connecting half, run by the parent below with `DBUS_SESSION_BUS_ADDRESS` pointing at a
/// private bus. Without the marker it does nothing, so a plain test run passes it by.
#[tokio::test]
async fn child_connects_over_the_session_bus_it_is_given() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        return;
    }
    let env = ClientEnv {
        links: vec![socket_link(), LinkChoice::Dbus],
    };
    let accounts = Accounts::connect(&env).await.expect("a link is reachable");
    let got = accounts
        .infer(&need(), DataClass::Public, Tier::Fast, request())
        .await;
    assert_eq!(got, Ok(reply()));
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_skips_a_dead_link_and_reaches_inferd_over_the_session_bus() {
    let bus = PrivateBus::start();
    let (fake, seen) = FakeInferd::new(Behaviour::Scripted(vec![Script {
        kind: RequestKind::Chat,
        steps: vec![ScriptStep::Emit(InferEvent::Finished(reply()))],
    }]));
    let daemon = bus.connect().await;
    fake.serve(&daemon).await;

    let status = tokio::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env_clear()
        .env(CHILD_MARKER, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .env("HOME", bus.scratch())
        .env("XDG_RUNTIME_DIR", bus.scratch())
        .status()
        .await
        .expect("run the child");
    assert!(status.success(), "the child failed: {status}");
    assert_eq!(seen.lock().expect("lock").opens.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_session_bus_the_dbus_link_is_unreachable() {
    let scratch = PrivateBus::start();
    let status = tokio::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "connect::child_finds_no_bus",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD_MARKER, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/no-bus")
        .env("HOME", scratch.scratch())
        .env("XDG_RUNTIME_DIR", scratch.scratch())
        .status()
        .await
        .expect("run the child");
    assert!(status.success(), "the child failed: {status}");
}

#[tokio::test]
async fn child_finds_no_bus() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        return;
    }
    let env = ClientEnv {
        links: vec![LinkChoice::Dbus],
    };
    let got = Accounts::connect(&env).await.map(|_| ());
    assert_eq!(
        got,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
}
