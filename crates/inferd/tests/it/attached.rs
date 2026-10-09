//! Engines the person already runs (`[engines.attached."<id>"]`), end to end: the real
//! `Inference1` object on a private bus over a lab engine (an OpenAI-compatible server on a
//! scratch Unix socket or a loopback port, `hosting::lab`), and the real `inferd` binary beside a
//! lab engine that is a process of its own. Nothing outside the scratch directories and loopback
//! ports the tests bind is touched; the only pids signalled are ones these tests started.

use crate::hosting;

use hosting::lab::Lab;
use hosting::rig::{Plan, World};
use hosting::{bus, entries};
use inferd::attached::{Attached, KeyFileProblem, NotReady, Place, Reach};
use inferd::serve::EngineHost;
use inferd::settings::{MyNetwork, Settings};
use inferd::startup::Cause;
use porter_client::InferSession;
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{DataClass, Locality, ModelId, Need, Tier, Tokens};
use porter_fake_servers::net::Bind;
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRefusal, InferReply,
    InferRequest, Knob, MessagePart, ModelRef, Reasoning, ReplyShape, Role as ChatRole, ToolChoice,
    ToolParallelism,
};
use rustix::process::{Pid, Signal, kill_process};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const ATTACHED: &str = entries::ATTACHED;

const KEY: &str = "sk-lab-1234-NOT-A-REAL-KEY";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// The lab engine as a process of its own. Run as a test it does nothing; run by the tests below
/// with `FAKE_LAB_DIR` set (as `attached::fake_lab_process`), it serves the attached entry on
/// `<dir>/lab.sock` (wanting `FAKE_LAB_KEY` when that is set) until it is killed.
#[test]
fn fake_lab_process() {
    let Ok(dir) = std::env::var("FAKE_LAB_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let key = std::env::var("FAKE_LAB_KEY").ok();
    // However it goes, no fake lab outlives a test by long.
    std::thread::spawn(|| {
        std::thread::sleep(porter_fake::GENEROUS * 3);
        std::process::exit(9);
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let _lab = Lab::start(
            &Bind::Socket(dir.clone()),
            "lab",
            &[entries::SERVED],
            key.as_deref(),
        )
        .await;
        std::fs::write(dir.join("ready"), b"").expect("ready");
        std::future::pending::<()>().await;
    });
}

/// A scratch directory, removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "at-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("lab.sock")
    }

    fn key_file(&self, text: &str, mode: u32) -> PathBuf {
        let path = self.0.join("lab.key");
        std::fs::write(&path, text).expect("write the key");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
        path
    }

    /// A lab engine on this directory's socket.
    async fn lab(&self, key: Option<&str>) -> Lab {
        Lab::start(
            &Bind::Socket(self.0.clone()),
            "lab",
            &[entries::SERVED],
            key,
        )
        .await
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A process this test started, killed (by pid) when the test ends however it ends.
struct Started(Child);

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The lab as a process; returns once it listens.
fn lab_process(dir: &Dir, key: Option<&str>) -> Started {
    let mut command = Command::new(std::env::current_exe().expect("this test binary"));
    command
        .args(["--exact", "attached::fake_lab_process", "--nocapture"])
        .env("FAKE_LAB_DIR", &dir.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(key) = key {
        command.env("FAKE_LAB_KEY", key);
    }
    let child = Started(command.spawn().expect("start the lab"));
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if dir.0.join("ready").exists() {
            return child;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    deadline.fail("the lab to listen");
}

/// Gone: no such process, or a zombie nobody has collected yet.
fn alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => false,
        Ok(stat) => !stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

fn signal(pid: u32, signal: Signal) {
    let pid = Pid::from_raw(i32::try_from(pid).expect("pid")).expect("a pid");
    kill_process(pid, signal).expect("signal a process this test started");
}

fn attach(reach: Reach, place: Place, key_file: Option<PathBuf>) -> Attached {
    Attached {
        id: ModelId::parse(entries::ATTACHED).expect("id"),
        reach,
        key_file,
        place,
        computer: None,
    }
}

fn on_socket(dir: &Dir, place: Place, key_file: Option<PathBuf>) -> Attached {
    attach(Reach::Socket(dir.socket()), place, key_file)
}

async fn world(attached: Vec<Attached>) -> World {
    World::start(Plan {
        catalog: vec![("attached.toml", entries::attached())],
        attached,
        ..Plan::default()
    })
    .await
}

fn model() -> ModelRef {
    ModelRef {
        account: porter_core::AccountId::parse("local").expect("id"),
        model: ModelId::parse(entries::ATTACHED).expect("id"),
    }
}

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn ask(class: DataClass, text: &str) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            parts: vec![MessagePart::Text(text.into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class,
        usage: Usage::Interactive,
        tools: vec![],
        control: ChatControl {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::One,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: vec![],
            scores: Knob::Off,
        },
    })
}

/// One turn of class `class`, the events it produced.
async fn turn(world: &World, class: DataClass, text: &str) -> Vec<InferEvent> {
    let mut session = world
        .accounts
        .session(&llm(), class, Tier::Balanced)
        .await
        .expect("open");
    let _ = session.send(ClientFrame::Request(ask(class, text))).await;
    let mut events = Vec::new();
    loop {
        let event = session.next().await.expect("an event");
        let done = matches!(event, InferEvent::Finished(_));
        events.push(event);
        if done {
            return events;
        }
    }
}

fn refused(refusal: InferRefusal) -> Vec<InferEvent> {
    vec![InferEvent::Finished(InferReply::Refused(refusal))]
}

fn routed(events: &[InferEvent]) -> &porter_infer::ServedBy {
    events
        .iter()
        .find_map(|event| match event {
            InferEvent::Routed(served) => Some(served),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a Routed event in {events:?}"))
}

fn said(events: &[InferEvent]) -> &str {
    let Some(InferEvent::Finished(InferReply::Chat(reply))) = events.last() else {
        panic!("a chat reply, got {events:?}");
    };
    &reply.text
}

async fn cause_of(world: &World) -> Cause {
    world
        .served
        .want(model())
        .await
        .expect_err("not ready")
        .cause
}

fn lines(world: &World) -> String {
    let held = world.log.lock().expect("lock");
    held.iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_streams_over_the_socket_and_nothing_is_started() {
    let dir = Dir::new();
    let lab = dir.lab(None).await;
    let world = world(vec![on_socket(&dir, Place::ThisDevice, None)]).await;

    // Mail data, floor "this computer": a `this-device` engine takes it.
    let events = turn(
        &world,
        DataClass::Mail,
        "Summarise: the meeting moved to noon.",
    )
    .await;
    let served = routed(&events);
    assert_eq!(
        (served.account.as_str(), served.model.as_str()),
        ("local", entries::ATTACHED)
    );
    assert_eq!(served.locality, Locality::OnDevice);
    assert_eq!(said(&events), "The lab says hello.");

    // The catalogue's own name on the wire, the prompt in the body.
    let chats = lab.chats();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0]["model"], entries::SERVED);
    assert!(chats[0].to_string().contains("the meeting moved to noon"));
    // The engine was looked at as the session opened and asked once for the turn.
    let paths: Vec<_> = lab.seen().into_iter().map(|seen| seen.path).collect();
    assert_eq!(
        paths.last().map(String::as_str),
        Some("/v1/chat/completions")
    );
    assert!(
        paths[..paths.len() - 1]
            .iter()
            .all(|path| path == "/v1/models"),
        "{paths:?}"
    );

    // Nothing was spawned, and the supervisor has no engine of that model at all.
    assert!(world.host.spawned.lock().expect("lock").is_empty());
    assert!(world.supervised.snapshot().states.is_empty());
    let audited = world.audit.entries();
    assert_eq!((audited.len(), audited[0].account.as_str()), (1, "local"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_streams_over_a_loopback_port_too() {
    let lab = Lab::start(&Bind::Loopback, "lab", &[entries::SERVED], None).await;
    let reach = Reach::Loopback {
        host: std::net::Ipv4Addr::LOCALHOST,
        port: model_http::Port(lab.port()),
    };
    let world = world(vec![attach(reach, Place::ThisDevice, None)]).await;
    let events = turn(&world, DataClass::Mail, "hello over loopback").await;
    assert_eq!(said(&events), "The lab says hello.");
    assert_eq!(lab.chats().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_way_an_engine_cannot_answer_is_a_typed_cause_and_the_session_is_unavailable() {
    let dir = Dir::new();
    let world = world(vec![on_socket(&dir, Place::MyNetwork, None)]).await;

    // The tunnel is down: nothing is at the path.
    assert_eq!(
        cause_of(&world).await,
        Cause::Attached(NotReady::SocketMissing { path: dir.socket() })
    );
    let events = turn(&world, DataClass::Public, "anyone?").await;
    assert_eq!(events, refused(InferRefusal::Unavailable));

    // A socket nobody listens on any more.
    drop(std::os::unix::net::UnixListener::bind(dir.socket()).expect("bind"));
    assert_eq!(cause_of(&world).await, Cause::Attached(NotReady::Refused));
    std::fs::remove_file(dir.socket()).expect("remove");

    // The engine serves another model.
    let lab = Lab::start(
        &Bind::Socket(dir.0.clone()),
        "lab",
        &["llama-3-8b", "qwen3-8b"],
        None,
    )
    .await;
    assert_eq!(
        cause_of(&world).await,
        Cause::Attached(NotReady::ModelAbsent {
            served: vec!["llama-3-8b".to_owned(), "qwen3-8b".to_owned()]
        })
    );
    // It is looked at again at the next open: the engine now serves the model, and the very next
    // session is answered (no restart, no rescan).
    lab.serve_models(&[entries::SERVED]);
    let events = turn(&world, DataClass::Public, "now?").await;
    assert_eq!(said(&events), "The lab says hello.");
    assert_eq!(
        world.served.attached().readiness(&model()),
        porter_infer::Readiness::Ready
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bearer_is_sent_from_the_key_file_and_without_it_the_engine_says_401() {
    let dir = Dir::new();
    let lab = dir.lab(Some(KEY)).await;

    // No key file: Unauthorized, and the session is unavailable.
    let without = world(vec![on_socket(&dir, Place::ThisDevice, None)]).await;
    assert_eq!(
        cause_of(&without).await,
        Cause::Attached(NotReady::Unauthorized { status: 401 })
    );
    assert_eq!(
        turn(&without, DataClass::Mail, "hi").await,
        refused(InferRefusal::Unavailable)
    );
    assert!(lab.chats().is_empty());

    // The key file, mode 0600: the probe and the chat both carry it.
    let key_file = dir.key_file(&format!("{KEY}\n"), 0o600);
    let with = world(vec![on_socket(&dir, Place::ThisDevice, Some(key_file))]).await;
    let events = turn(&with, DataClass::Mail, "hi").await;
    assert_eq!(said(&events), "The lab says hello.");
    let sent: Vec<_> = lab
        .seen()
        .into_iter()
        .filter(|seen| seen.status == 200)
        .map(|seen| (seen.path, seen.authorization))
        .collect();
    let bearer = Some(format!("Bearer {KEY}"));
    assert_eq!(
        sent.last(),
        Some(&("/v1/chat/completions".to_owned(), bearer.clone()))
    );
    assert!(
        sent[..sent.len() - 1]
            .iter()
            .all(|(path, auth)| path == "/v1/models" && *auth == bearer),
        "{sent:?}"
    );
    // The token is in no line inferd logged.
    assert!(!lines(&without).contains(KEY) && !lines(&with).contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_file_others_can_read_is_refused_and_no_request_carries_the_token() {
    let dir = Dir::new();
    let lab = dir.lab(Some(KEY)).await;
    let key_file = dir.key_file(&format!("{KEY}\n"), 0o644);
    let world = world(vec![on_socket(&dir, Place::ThisDevice, Some(key_file))]).await;
    assert_eq!(
        cause_of(&world).await,
        Cause::Attached(NotReady::KeyFile(KeyFileProblem::NotPrivate {
            mode: 0o644
        }))
    );
    assert_eq!(
        turn(&world, DataClass::Mail, "hi").await,
        refused(InferRefusal::Unavailable)
    );
    assert_eq!(
        lab.seen(),
        vec![],
        "the engine was never asked, so no token went"
    );
    // What was logged says why, in the person's terms, and holds nothing of the token.
    let said = lines(&world);
    assert!(said.contains("readable by others"), "{said}");
    assert!(!said.contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn this_device_takes_on_device_only_data_and_my_network_does_not_unless_the_setting_allows() {
    let dir = Dir::new();
    let lab = dir.lab(None).await;
    let world = world(vec![on_socket(&dir, Place::MyNetwork, None)]).await;

    // The default setting: Mail may go to this computer only, and the lab is another machine.
    let before = turn(&world, DataClass::Mail, "private").await;
    assert_eq!(
        before,
        refused(InferRefusal::RequiresCloud(DataClass::Mail))
    );
    assert!(lab.chats().is_empty(), "nothing was sent");

    // The card says where the data goes, and costs and asks for nothing.
    let listed = world.served.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].card.locality, Locality::LocalNetwork);
    assert_eq!(listed[0].card.billing, porter_core::Billing::Free);
    assert_eq!(listed[0].spend, porter_infer::SpendVerdict::Within);
    assert!(matches!(
        listed[0].permission,
        porter_core::consent::Verdict::Granted { .. }
    ));

    // A class whose floor says "my machines" is let through either way.
    let mut settings: Settings = (*world.served.settings()).clone();
    settings
        .policy
        .floors
        .retain(|row| row.class != DataClass::Notes);
    settings.policy.floors.push(porter_infer::ClassFloor {
        class: DataClass::Notes,
        floor: porter_infer::Floor::LocalNetwork,
    });
    world.served.apply(settings.clone());
    let notes = turn(&world, DataClass::Notes, "a note").await;
    assert_eq!(routed(&notes).locality, Locality::LocalNetwork);

    // `ai.attached.my_network` on: the on-device floor of Mail lets it through.
    settings.my_network = MyNetwork::On;
    world.served.apply(settings);
    let after = turn(&world, DataClass::Mail, "private").await;
    assert_eq!(routed(&after).locality, Locality::LocalNetwork);
    assert_eq!(said(&after), "The lab says hello.");
    assert_eq!(lab.chats().len(), 2);

    // It never counts as cloud: local-only stays on throughout, and no cloud spend is recorded.
    assert_eq!(
        world.served.settings().policy.local_only,
        porter_infer::LocalOnly::On
    );
    assert!(world.served.usage_of(&hosting::rig::test_app()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_eviction_pass_leaves_the_attached_engines_process_alive_and_still_ready() {
    let dir = Dir::new();
    let lab = lab_process(&dir, None);
    let pid = lab.0.id();
    // Two supervised models that do not fit the GPU together, and the attached one.
    let big = |name: &str| {
        entries::chat()
            .replace("tiny-chat", name)
            .replace("weights_mib = 100", "weights_mib = 9000")
    };
    let world = World::start(Plan {
        catalog: vec![
            ("attached.toml", entries::attached()),
            ("big-a.toml", big("big-a")),
            ("big-b.toml", big("big-b")),
        ],
        attached: vec![on_socket(&dir, Place::MyNetwork, None)],
        ..Plan::default()
    })
    .await;
    let spec_of = |id: &str| {
        world
            .models
            .iter()
            .find(|model| model.entry.id.0 == id)
            .expect("model")
            .spec
            .id
            .clone()
    };
    let (a, b) = (spec_of("big-a"), spec_of("big-b"));
    assert!(
        world
            .models
            .iter()
            .all(|model| model.entry.id.0 != ATTACHED),
        "an attached id is not also one inferd runs"
    );

    let first = tokio::time::timeout(porter_fake::GENEROUS, world.supervised.want(&a)).await;
    assert_eq!(first, Ok(Ok(())), "the first engine starts");
    // The first engine goes idle: a turn in the last probe window is never a victim.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let second = tokio::time::timeout(porter_fake::GENEROUS, world.supervised.want(&b)).await;
    assert_eq!(second, Ok(Ok(())), "the second engine starts");
    let states = world.supervised.snapshot().states;
    assert!(matches!(
        states.get(&a),
        Some(engine_supervisor::EngineState::Stopped)
    ));
    assert!(matches!(
        states.get(&b),
        Some(engine_supervisor::EngineState::Ready { .. })
    ));
    assert_eq!(
        states.len(),
        2,
        "the attached engine is in no supervisor: {states:?}"
    );
    // Only the idle supervised engine was stopped: the host was never asked about the attached one.
    assert_eq!(*world.host.stopped.lock().expect("lock"), vec![a.clone()]);
    assert!(
        !world
            .host
            .spawned
            .lock()
            .expect("lock")
            .iter()
            .any(|id| id.0.starts_with("attached:"))
    );

    // The fake server's process is alive and answers, and the daemon still finds it ready.
    assert!(alive(pid));
    assert_eq!(world.served.want(model()).await, Ok(()));
    assert_eq!(
        world.served.attached().readiness(&model()),
        porter_infer::Readiness::Ready
    );
    assert!(alive(pid));
}

/// The real inferd on a private bus with `config`, a scratch XDG and the catalog of `catalog`
/// (file name, text) in its user catalog directory.
fn real_inferd(bus: &bus::PrivateBus, config: &str, catalog: &[(&str, String)]) -> Started {
    let scratch = bus.scratch();
    let file = scratch.join("inferd.toml");
    std::fs::write(&file, config).expect("config");
    let catalog_dir = scratch.join("data").join("stoker").join("catalog");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
    for (name, text) in catalog {
        std::fs::write(catalog_dir.join(name), text).expect("catalog file");
    }
    Started(
        Command::new(env!("CARGO_BIN_EXE_inferd"))
            .env_clear()
            .env("HOME", scratch)
            .env("XDG_RUNTIME_DIR", scratch)
            .env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("XDG_DATA_HOME", scratch.join("data"))
            .env("XDG_STATE_HOME", scratch.join("state"))
            .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
            // A Tailscale nobody serves: this computer's real one is never asked.
            .env("INFERD_TAILSCALE_SOCKET", scratch.join("no-tailscale.sock"))
            .arg("--config")
            .arg(&file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("inferd binary"),
    )
}

fn exit_within(child: &mut Started, within: Duration) -> Option<i32> {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if let Some(status) = child.0.try_wait().expect("wait") {
            return Some(status.code().unwrap_or(-1));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn stderr_of(mut child: Started) -> String {
    use std::io::Read;
    let mut text = String::new();
    if let Some(mut pipe) = child.0.stderr.take() {
        let _ = pipe.read_to_string(&mut text);
    }
    text
}

const NO_PROBE: &str = "[probe]\nollama = []\nllama_cpp = []\nlm_studio = []\n";

#[tokio::test(flavor = "multi_thread")]
async fn sigterm_to_the_real_inferd_leaves_the_attached_engines_process_alive() {
    let bus = bus::PrivateBus::start();
    let dir = Dir::new();
    let lab = lab_process(&dir, None);
    let pid = lab.0.id();
    let config = format!(
        "{NO_PROBE}\n[engines.attached.\"{ATTACHED}\"]\nsocket = \"{}\"\nwhere = \"my-network\"\n",
        dir.socket().display()
    );
    let mut inferd = real_inferd(&bus, &config, &[("attached.toml", entries::attached())]);
    let client = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&client).await.expect("dbus");
    let name = zbus::names::BusName::try_from(porter_dbus::INFERENCE_BUS).expect("bus name");
    let until = Instant::now() + Duration::from_secs(20);
    while !dbus.name_has_owner(name.clone()).await.expect("ask") {
        assert!(Instant::now() < until, "inferd never took its name");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(alive(pid));
    signal(inferd.0.id(), Signal::TERM);
    assert_eq!(exit_within(&mut inferd, Duration::from_secs(15)), Some(0));
    assert!(
        alive(pid),
        "inferd's shutdown did not reach the engine it never started"
    );
    // And it still answers: nothing closed its socket.
    let target = inferd::attached::Target {
        reach: Reach::Socket(dir.socket()),
        key: None,
        place: Place::MyNetwork,
        computer: None,
    };
    assert_eq!(
        inferd::attached::probe(&target, entries::SERVED).await,
        Ok(())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_real_inferd_refuses_to_start_on_a_bad_attached_table_and_says_which() {
    let table = [
        (
            format!("[engines.attached.\"{ATTACHED}\"]\nurl = \"http://10.1.2.3:8000\"\nwhere = \"my-network\"\n"),
            "not loopback",
        ),
        (
            format!("[engines.attached.\"{ATTACHED}\"]\nsocket = \"/run/lab.sock\"\n"),
            "`where` is required",
        ),
        (
            "[engines.attached.\"not-in-the-catalogue\"]\nsocket = \"/run/lab.sock\"\nwhere = \"this-device\"\n".to_owned(),
            "no model of that id in the catalogue",
        ),
        (
            "[engines.attached.\"tiny-chat\"]\nsocket = \"/run/lab.sock\"\nwhere = \"this-device\"\n".to_owned(),
            "serving is not attached",
        ),
        (
            format!("[engines.attached.\"{ATTACHED}\"]\nsocket = \"/run/lab.sock\"\nurl = \"http://127.0.0.1:1\"\nwhere = \"this-device\"\n"),
            "not both",
        ),
    ];
    for (tables, why) in table {
        let bus = bus::PrivateBus::start();
        let mut inferd = real_inferd(
            &bus,
            &format!("{NO_PROBE}\n{tables}"),
            &[
                ("attached.toml", entries::attached()),
                ("tiny-chat.toml", entries::chat()),
            ],
        );
        assert_eq!(
            exit_within(&mut inferd, Duration::from_secs(20)),
            Some(1),
            "{tables}"
        );
        let said = stderr_of(inferd);
        assert!(said.contains(why), "{why:?} in {said}");
    }
}
