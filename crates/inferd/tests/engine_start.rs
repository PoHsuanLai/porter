//! A failed engine start is a typed failure, not `NotReady` until a client gives up. The daemon
//! runs on a private bus with real child processes for engines: a script (written by the test,
//! in a scratch directory) runs this test binary again as the fake engine, which exits at start,
//! binds its socket and dies, binds and answers `/health`, or never answers. No model, no GPU, no
//! network, nothing outside the scratch directories.

mod hosting;

use hosting::entries;
use hosting::rig::{Plan, Processes, World};
use inferd::startup::{Cause, Level};
use porter_client::InferSession;
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{DataClass, Need, Tier, Tokens};
use porter_dbus::{Details, InferenceProxy, need_to_dbus};
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferReply, InferRequest, Knob,
    MessagePart, ModelError, Reasoning, ReplyShape, Role as ChatRole, ToolChoice, ToolParallelism,
};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The fake engine. Run as a test it does nothing; run by the script below, with `FAKE_ENGINE`
/// set, it plays the engine named there on the socket `FAKE_SOCK`.
#[test]
fn fake_engine() {
    let Ok(mode) = std::env::var("FAKE_ENGINE") else {
        return;
    };
    let socket = PathBuf::from(std::env::var("FAKE_SOCK").expect("FAKE_SOCK"));
    let dir = PathBuf::from(std::env::var("FAKE_DIR").expect("FAKE_DIR"));
    // However it goes, no fake engine outlives a test by long.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        std::process::exit(9);
    });
    match mode.as_str() {
        "exit3" => {
            eprintln!("Triton compile failed: out of resources");
            std::process::exit(3);
        }
        "bind_then_die" => die_after_binding(&socket),
        // The first run leaves a socket behind and dies; the next one needs the path free.
        "flaky" => {
            let runs = std::fs::read_to_string(dir.join("runs")).unwrap_or_default();
            if runs.lines().count() <= 1 {
                die_after_binding(&socket);
            }
            serve(&socket);
        }
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        other => panic!("no fake engine {other}"),
    }
}

fn die_after_binding(socket: &Path) -> ! {
    let listener = UnixListener::bind(socket).expect("bind");
    eprintln!("died after binding {}", socket.display());
    // No unlink, no drop: the socket file stays, as after a crash.
    std::mem::forget(listener);
    std::process::exit(5);
}

fn serve(socket: &Path) -> ! {
    let listener = UnixListener::bind(socket).expect("bind: Address already in use");
    for stream in listener.incoming().flatten() {
        let mut stream = stream;
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
        );
    }
    std::process::exit(0);
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch directory and the program that is llama-server in it: a script that notes each run
/// in `runs` and becomes the fake engine in `mode` on the socket it is told (`--host <socket>`).
struct Engine {
    dir: PathBuf,
    program: PathBuf,
}

impl Engine {
    fn new(mode: &str) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "es-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        let exe = std::env::current_exe().expect("this test binary");
        let program = dir.join("engine.sh");
        let script = format!(
            "#!/bin/sh\necho run >> '{dir}/runs'\nprev=\"\"\nfor a in \"$@\"; do\n  \
             if [ \"$prev\" = \"--host\" ]; then sock=\"$a\"; fi\n  prev=\"$a\"\ndone\n\
             FAKE_ENGINE='{mode}' FAKE_SOCK=\"$sock\" FAKE_DIR='{dir}' exec '{exe}' --exact fake_engine --nocapture\n",
            dir = dir.display(),
            exe = exe.display(),
        );
        std::fs::write(&program, script).expect("script");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        Self { dir, program }
    }

    /// How many times the program was started.
    fn runs(&self) -> usize {
        std::fs::read_to_string(self.dir.join("runs")).map_or(0, |text| text.lines().count())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Supervisor timing for a test: probes every 50 ms, three attempts 100 ms and 200 ms apart, a
/// pause of 3 s after the last, and `start_timeout` to be ready.
fn timing(start_timeout: Duration) -> engine_supervisor::SupervisorConfig {
    engine_supervisor::SupervisorConfig {
        start_timeout,
        probe_every: Duration::from_millis(50),
        backoff: (Duration::from_millis(100), Duration::from_millis(3000)),
        ..engine_supervisor::SupervisorConfig::default()
    }
}

async fn world(engine: &Engine, start_timeout: Duration) -> World {
    World::start(Plan {
        catalog: vec![("tiny-chat.toml", entries::chat())],
        processes: Some(Processes {
            program: engine.program.clone(),
        }),
        supervisor: Some(timing(start_timeout)),
        ..Plan::default()
    })
    .await
}

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn chat_request() -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            parts: vec![MessagePart::Text("hello".into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class: DataClass::Notes,
        usage: Usage::Interactive,
        tools: vec![],
        control: ChatControl {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::One,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: vec![],
        },
    })
}

/// Opens a session and asks for a turn; the events up to `Finished`, and how long it took. Fails
/// the test when the turn is not over in `within`.
async fn open_and_ask(world: &World, within: Duration) -> (Vec<InferEvent>, Duration) {
    let begun = Instant::now();
    let run = async {
        let mut session = world
            .accounts
            .session(&llm(), DataClass::Notes, Tier::Balanced)
            .await
            .expect("open");
        // The engine is asked for at Open: a failure can end the session before the request is
        // sent, so a closed session here is not an error, its events are still to be read.
        let _ = session.send(ClientFrame::Request(chat_request())).await;
        let mut events = Vec::new();
        loop {
            let event = session.next().await.expect("an event");
            let done = matches!(event, InferEvent::Finished(_));
            events.push(event);
            if done {
                return events;
            }
        }
    };
    let events = tokio::time::timeout(within, run)
        .await
        .unwrap_or_else(|_| panic!("the turn was not over in {within:?}"));
    (events, begun.elapsed())
}

fn not_ready(events: &[InferEvent]) -> bool {
    matches!(
        events.last(),
        Some(InferEvent::Finished(InferReply::Failed(
            ModelError::NotReady
        )))
    )
}

async fn prepare(world: &World) -> String {
    InferenceProxy::new(&world.client)
        .await
        .expect("proxy")
        .prepare(&need_to_dbus(&llm()), "notes", "fast", &Details::new())
        .await
        .expect("prepare")
}

fn the_failure(world: &World) -> inferd::supervise::Failure {
    world
        .supervised
        .snapshot()
        .failures
        .values()
        .next()
        .cloned()
        .expect("a failure on record")
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_engine_that_exits_at_start_fails_prepare_and_open_with_its_cause() {
    let engine = Engine::new("exit3");
    // The readiness timeout is the default 180 s: the failure must not wait for it.
    let world = world(&engine, Duration::from_secs(180)).await;

    assert_eq!(prepare(&world).await, "loading");
    let (events, took) = open_and_ask(&world, Duration::from_secs(5)).await;
    assert!(not_ready(&events), "{events:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");

    // The cause carries the exit status and the engine's last line.
    let failure = the_failure(&world);
    let Cause::Exited { code, tail } = &failure.cause else {
        panic!("{:?}", failure.cause);
    };
    assert_eq!(code.0, 3);
    assert_eq!(
        tail.short().as_deref(),
        Some("Triton compile failed: out of resources")
    );
    let said = failure.cause.to_string();
    assert!(
        said.contains("status 3") && said.contains("Triton compile failed"),
        "{said}"
    );
    // The same lines are in inferd's log at warn, the tail under them.
    let logged = world.log.lock().expect("lock").clone();
    let warned = logged
        .iter()
        .find(|(level, text)| *level == Level::Warn && text.contains("status 3"))
        .unwrap_or_else(|| panic!("no warn line: {logged:?}"));
    assert!(warned.1.contains("| Triton compile failed"), "{}", warned.1);

    // The machine's three attempts are over; then a request is failed at once, and none starts
    // the engine again inside the pause.
    until("the third attempt failed", || engine.runs() == 3).await;
    until("given up", || {
        matches!(
            world.supervised.snapshot().states.values().next(),
            Some(engine_supervisor::EngineState::Failed(_))
        )
    })
    .await;
    assert_eq!(prepare(&world).await, "unavailable");
    for _ in 0..3 {
        let (events, took) = open_and_ask(&world, Duration::from_secs(5)).await;
        assert!(
            not_ready(&events)
                || matches!(
                    events.last(),
                    Some(InferEvent::Finished(InferReply::Refused(_)))
                ),
            "{events:?}"
        );
        assert!(took < Duration::from_secs(1), "{took:?}");
    }
    assert_eq!(engine.runs(), 3, "no request started it inside the pause");

    // The pause is over: the next request tries once more, and fails the same way.
    tokio::time::sleep(Duration::from_millis(3100)).await;
    let (events, _) = open_and_ask(&world, Duration::from_secs(5)).await;
    assert!(not_ready(&events), "{events:?}");
    assert!(engine.runs() >= 4, "{}", engine.runs());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_socket_is_removed_and_the_restart_comes_up() {
    let engine = Engine::new("flaky");
    let world = world(&engine, Duration::from_secs(30)).await;
    let socket = world.models[0].socket.0.clone();

    // The first run binds the socket and dies; the waiter is told at once, with that exit.
    let (events, took) = open_and_ask(&world, Duration::from_secs(5)).await;
    assert!(not_ready(&events), "{events:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");
    let failure = the_failure(&world);
    let Cause::Exited { code, tail } = &failure.cause else {
        panic!("{:?}", failure.cause);
    };
    assert_eq!(code.0, 5);
    assert!(
        tail.short()
            .is_some_and(|line| line.starts_with("died after binding")),
        "{tail:?}"
    );

    // The machine starts it again: the host takes the stale socket away, so the second run can
    // bind (it dies with "Address already in use" otherwise) and is ready.
    until("ready", || {
        matches!(
            world.supervised.snapshot().states.values().next(),
            Some(engine_supervisor::EngineState::Ready { .. })
        )
    })
    .await;
    assert_eq!(engine.runs(), 2);
    assert_eq!(prepare(&world).await, "ready");
    assert!(socket.exists());
    // A ready engine has no failure on record.
    assert!(world.supervised.snapshot().failures.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_engine_that_never_answers_fails_when_its_time_is_up_not_when_the_client_is() {
    let engine = Engine::new("hang");
    let world = world(&engine, Duration::from_millis(600)).await;
    let (events, took) = open_and_ask(&world, Duration::from_secs(10)).await;
    assert!(not_ready(&events), "{events:?}");
    assert!(took >= Duration::from_millis(500), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    let failure = the_failure(&world);
    assert!(
        matches!(failure.cause, Cause::NeverReady { .. }),
        "{:?}",
        failure.cause
    );
    assert!(
        world
            .log
            .lock()
            .expect("lock")
            .iter()
            .any(|(level, text)| *level == Level::Warn && text.contains("did not become ready")),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_at_the_socket_path_is_kept_and_the_engine_is_not_started() {
    let engine = Engine::new("exit3");
    let world = world(&engine, Duration::from_secs(180)).await;
    let socket = world.models[0].socket.0.clone();
    std::fs::write(&socket, b"not a socket").expect("write");

    let (events, took) = open_and_ask(&world, Duration::from_secs(5)).await;
    assert!(not_ready(&events), "{events:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");
    let failure = the_failure(&world);
    assert_eq!(
        failure.cause,
        Cause::SocketPathTaken {
            path: socket.clone()
        }
    );
    assert_eq!(std::fs::read(&socket).expect("kept"), b"not a socket");
    assert_eq!(engine.runs(), 0, "the program was never run");
    let logged = world.log.lock().expect("lock").clone();
    assert!(
        logged.iter().any(|(level, text)| *level == Level::Error
            && text.contains(&socket.display().to_string())
            && text.contains("not a socket")),
        "{logged:?}"
    );
}
