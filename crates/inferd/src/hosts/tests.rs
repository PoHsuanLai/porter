use super::*;
use crate::testkit::Scratch;
use engine_supervisor::{EnvPair, GpuAccess, Network, ProgramPath, Sandbox};
use model_catalog::EngineArg;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

fn unit(program: &str, args: &[&str]) -> UnitSpec {
    UnitSpec {
        program: ProgramPath(PathBuf::from(program)),
        args: args.iter().map(|a| EngineArg((*a).into())).collect(),
        env: vec![EnvPair {
            name: "HF_HUB_OFFLINE".into(),
            value: "1".into(),
        }],
        sandbox: Sandbox {
            network: Network::None,
            read: vec![],
            write: vec![],
            gpu: GpuAccess::Absent,
            memory_max: MiB(0),
        },
    }
}

fn id(name: &str) -> EngineId {
    EngineId(name.into())
}

#[test]
fn the_total_is_the_first_line_of_the_csv() {
    let cases = [
        ("16303\n", Ok(MiB(16303))),
        ("16303", Ok(MiB(16303))),
        (" 12288 \n24576\n", Ok(MiB(12288))),
        ("", Err(GpuError::Unreadable)),
        ("N/A\n", Err(GpuError::Unreadable)),
        ("16303 MiB\n", Err(GpuError::Unreadable)),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_total(text), expected, "{text:?}");
    }
}

#[tokio::test]
async fn nvidia_smi_that_is_not_there_is_unavailable() {
    let smi = NvidiaSmi::new(PathBuf::from("/nonexistent/nvidia-smi"));
    assert_eq!(smi.memory().await, Err(GpuError::Unavailable));
}

#[tokio::test]
async fn a_child_process_is_started_stopped_and_its_exit_seen() {
    let host = ProcessHost::new();
    host.spawn(&id("a"), &unit("sleep", &["30"]))
        .await
        .expect("spawn");
    host.stop(&id("a")).await.expect("stop");
    // A killed process has no exit code of its own.
    assert_eq!(host.exited(&id("a")).await, ExitCode(-1));
    // It is stopped once; the second stop finds nothing to stop.
    assert_eq!(host.stop(&id("a")).await, Err(HostError::NotRunning));
}

#[tokio::test]
async fn a_process_that_ends_on_its_own_reports_its_code() {
    let host = ProcessHost::new();
    host.spawn(&id("b"), &unit("sh", &["-c", "exit 3"]))
        .await
        .expect("spawn");
    assert_eq!(host.exited(&id("b")).await, ExitCode(3));
}

#[tokio::test]
async fn a_program_that_is_not_there_is_refused_and_an_unknown_engine_has_not_exited_well() {
    let host = ProcessHost::new();
    assert_eq!(
        host.spawn(&id("c"), &unit("/nonexistent/engine", &[]))
            .await,
        Err(HostError::Refused)
    );
    assert_eq!(host.exited(&id("never-started")).await, ExitCode(-1));
}

/// A socket that answers each connection with `response` and closes it.
fn answering(dir: &Scratch, name: &str, response: &'static str) -> PathBuf {
    let path = dir.path().join(name);
    let listener = UnixListener::bind(&path).expect("bind");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    path
}

#[tokio::test]
async fn the_health_probe_reads_the_status_off_the_engines_socket() {
    let dir = Scratch::new("hosts-health");
    let ok = answering(
        &dir,
        "ok.sock",
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    let loading = answering(
        &dir,
        "loading.sock",
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    let probe = HealthProbe::new([
        (id("ok"), ok),
        (id("loading"), loading),
        (id("gone"), dir.path().join("nobody-listens.sock")),
    ]);
    assert_eq!(probe.probe(&id("ok")).await, Probe::Ready);
    assert_eq!(probe.probe(&id("loading")).await, Probe::Loading);
    assert_eq!(probe.probe(&id("gone")).await, Probe::Down);
    assert_eq!(probe.probe(&id("unknown")).await, Probe::Down);
}

#[tokio::test]
async fn a_speech_host_is_probed_with_hello_not_with_http() {
    use crate::speech_host::{FakeSpeechHost, Words};
    let dir = Scratch::new("hosts-speech-probe");
    let host_socket = dir.path().join("host.sock");
    let host = FakeSpeechHost::start(&host_socket, Words(vec![]));
    // An engine that speaks HTTP, probed as a speech host, does not say Hello.
    let http = answering(
        &dir,
        "http.sock",
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    let probe = HealthProbe::new([
        (id("speech"), host_socket.clone()),
        (id("http-as-speech"), http.clone()),
        (id("gone"), dir.path().join("nobody-listens.sock")),
        (id("http"), http),
    ])
    .speech_hosts([id("speech"), id("http-as-speech"), id("gone")]);
    assert_eq!(probe.probe(&id("speech")).await, Probe::Ready);
    assert_eq!(host.seen.lock().expect("lock").hellos, 1);
    assert_eq!(probe.probe(&id("http-as-speech")).await, Probe::Down);
    assert_eq!(probe.probe(&id("gone")).await, Probe::Down);
    // The same socket under the plain probe is asked for `/health`, as before.
    assert_eq!(probe.probe(&id("http")).await, Probe::Ready);
    host.crash();
}

#[tokio::test]
async fn a_child_process_gets_the_environment_its_unit_names() {
    let dir = Scratch::new("hosts-env");
    let out = dir.path().join("env.txt");
    let mut spec = unit(
        "sh",
        &[
            "-c",
            &format!("printf '%s' \"$LD_LIBRARY_PATH\" > {}", out.display()),
        ],
    );
    spec.env.push(EnvPair {
        name: "LD_LIBRARY_PATH".into(),
        value: "/opt/sherpa/lib".into(),
    });
    let host = ProcessHost::new();
    host.spawn(&id("env"), &spec).await.expect("spawn");
    assert_eq!(host.exited(&id("env")).await, ExitCode(0));
    assert_eq!(
        std::fs::read_to_string(out).expect("written"),
        "/opt/sherpa/lib"
    );
}

/// A host that checks `socket` for the engine `name`, and the script that records whether the
/// socket was there when it ran (`seen`: `present` or `absent`).
fn watched(dir: &Scratch, name: &str, socket: PathBuf) -> (ProcessHost, UnitSpec, PathBuf) {
    let seen = dir.path().join(format!("{name}.seen"));
    let script = format!(
        "if [ -e '{s}' ]; then echo present > '{o}'; else echo absent > '{o}'; fi",
        s = socket.display(),
        o = seen.display()
    );
    let host = ProcessHost::new().with_sockets([(id(name), socket)]);
    (host, unit("sh", &["-c", &script]), seen)
}

#[tokio::test]
async fn what_a_process_said_last_is_in_the_tail_when_its_exit_is_seen() {
    let host = ProcessHost::new();
    host.spawn(
        &id("loud"),
        &unit(
            "sh",
            &[
                "-c",
                "echo 'starting' >&2; echo 'Triton compile failed' >&2; exit 3",
            ],
        ),
    )
    .await
    .expect("spawn");
    assert_eq!(host.exited(&id("loud")).await, ExitCode(3));
    assert_eq!(
        host.diagnostics().tail(&id("loud")).0,
        vec!["starting".to_owned(), "Triton compile failed".to_owned()]
    );
}

#[tokio::test]
async fn a_process_that_says_a_great_deal_is_read_to_the_end_and_kept_to_the_cap() {
    let host = ProcessHost::new();
    // 4 000 lines: far more than a pipe holds, so the process only ends if it is being read.
    host.spawn(
        &id("chatty"),
        &unit(
            "sh",
            &[
                "-c",
                "i=0; while [ $i -lt 4000 ]; do echo \"line $i of the noise\" >&2; i=$((i+1)); done",
            ],
        ),
    )
    .await
    .expect("spawn");
    assert_eq!(host.exited(&id("chatty")).await, ExitCode(0));
    let tail = host.diagnostics().tail(&id("chatty"));
    assert_eq!(tail.0.len(), crate::startup::TAIL_LINES);
    assert_eq!(
        tail.0.last().map(String::as_str),
        Some("line 3999 of the noise")
    );
}

#[tokio::test]
async fn a_socket_path_that_is_too_long_is_refused_before_anything_is_spawned() {
    let dir = Scratch::new("hosts-long");
    let deep = dir.path().join("d".repeat(120)).join("e.sock");
    let (host, spec, seen) = watched(&dir, "long", deep.clone());
    assert_eq!(
        host.spawn(&id("long"), &spec).await,
        Err(HostError::Refused)
    );
    assert_eq!(
        host.diagnostics().refusal(&id("long")),
        Some(Cause::SocketPathTooLong {
            len: deep.as_os_str().len(),
            path: deep,
        })
    );
    assert!(!seen.exists(), "the program was not run");
}

#[tokio::test]
async fn a_regular_file_at_the_socket_path_is_refused_and_kept() {
    let dir = Scratch::new("hosts-file");
    let socket = dir.path().join("e.sock");
    std::fs::write(&socket, b"not yours to delete").expect("write");
    let (host, spec, seen) = watched(&dir, "file", socket.clone());
    assert_eq!(
        host.spawn(&id("file"), &spec).await,
        Err(HostError::Refused)
    );
    assert_eq!(
        host.diagnostics().refusal(&id("file")),
        Some(Cause::SocketPathTaken {
            path: socket.clone()
        })
    );
    assert_eq!(
        std::fs::read(&socket).expect("kept"),
        b"not yours to delete"
    );
    assert!(!seen.exists(), "the program was not run");
}

#[tokio::test]
async fn a_stale_socket_is_gone_by_the_time_the_program_runs() {
    let dir = Scratch::new("hosts-stale");
    let socket = dir.path().join("e.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket).expect("bind"));
    assert!(socket.exists());
    let (host, spec, seen) = watched(&dir, "stale", socket);
    host.spawn(&id("stale"), &spec).await.expect("spawn");
    assert_eq!(host.exited(&id("stale")).await, ExitCode(0));
    assert_eq!(std::fs::read_to_string(seen).expect("ran"), "absent\n");
    assert_eq!(host.diagnostics().refusal(&id("stale")), None);
}

#[tokio::test]
async fn a_program_that_cannot_be_run_says_which() {
    let host = ProcessHost::new();
    assert_eq!(
        host.spawn(&id("nope"), &unit("/nonexistent/engine", &[]))
            .await,
        Err(HostError::Refused)
    );
    assert_eq!(
        host.diagnostics().refusal(&id("nope")),
        Some(Cause::CannotSpawn {
            program: PathBuf::from("/nonexistent/engine"),
            kind: std::io::ErrorKind::NotFound,
        })
    );
}

// The process group: an engine's own children are ended with it.

/// An engine that forks a grandchild which ignores `SIGTERM` and sleeps, and records its pid in
/// `pidfile`; the engine itself waits for it.
fn forking(pidfile: &Path) -> UnitSpec {
    unit(
        "sh",
        &[
            "-c",
            &format!(
                "(trap '' TERM; exec sleep 300) & echo $! > '{}'; wait",
                pidfile.display()
            ),
        ],
    )
}

/// The pid in `pidfile`, once the engine has written it.
async fn pid_in(pidfile: &Path) -> i32 {
    for _ in 0..200 {
        if let Some(pid) = std::fs::read_to_string(pidfile)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no pid in {}", pidfile.display());
}

/// Whether the process is gone (a zombie that nobody has collected yet is gone too).
fn gone(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

async fn gone_soon(pid: i32) -> bool {
    for _ in 0..160 {
        if gone(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test]
async fn stopping_an_engine_ends_its_grandchild_even_one_that_ignores_sigterm() {
    let dir = Scratch::new("hosts-group-stop");
    let pidfile = dir.path().join("pid");
    let host = ProcessHost::new().with_grace(Duration::from_millis(300));
    host.spawn(&id("g"), &forking(&pidfile))
        .await
        .expect("spawn");
    let grandchild = pid_in(&pidfile).await;
    assert!(!gone(grandchild));
    host.stop(&id("g")).await.expect("stop");
    // The exit is reported once the group has been killed, so the next engine finds the memory
    // free (a killed process is a moment dying, and a zombie waits for its new parent).
    host.exited(&id("g")).await;
    assert!(
        gone_soon(grandchild).await,
        "gone once the exit is reported"
    );
}

#[tokio::test]
async fn an_engine_that_exits_by_itself_takes_what_it_left_running_with_it() {
    let dir = Scratch::new("hosts-group-left");
    let pidfile = dir.path().join("pid");
    let host = ProcessHost::new();
    host.spawn(
        &id("l"),
        &unit(
            "sh",
            &[
                "-c",
                &format!("sleep 300 & echo $! > '{}'; exit 4", pidfile.display()),
            ],
        ),
    )
    .await
    .expect("spawn");
    assert_eq!(host.exited(&id("l")).await, ExitCode(4));
    let grandchild = pid_in(&pidfile).await;
    assert!(gone_soon(grandchild).await, "the orphan was swept");
}

#[tokio::test]
async fn dropping_the_host_ends_every_group_still_running() {
    let dir = Scratch::new("hosts-group-drop");
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    let host = ProcessHost::new();
    host.spawn(&id("a"), &forking(&a)).await.expect("spawn");
    host.spawn(&id("b"), &forking(&b)).await.expect("spawn");
    let (pa, pb) = (pid_in(&a).await, pid_in(&b).await);
    drop(host);
    assert!(gone_soon(pa).await && gone_soon(pb).await);
}

#[tokio::test(start_paused = false)]
async fn the_supervisor_evicting_an_engine_ends_its_grandchild() {
    use crate::supervise::{Ports, Supervised};
    use engine_supervisor::{FakeGpu, FakeReadyProbe, GpuMemory, SupervisorConfig};
    let dir = Scratch::new("hosts-group-evict");
    let mut specs: Vec<_> = crate::testkit::models(&dir)
        .into_iter()
        .take(2)
        .map(|m| m.spec)
        .collect();
    let files: Vec<PathBuf> = ["a", "b"].iter().map(|n| dir.path().join(n)).collect();
    for (spec, file) in specs.iter_mut().zip(&files) {
        spec.need = MiB(9_000);
        spec.unit = forking(file);
    }
    let (first, second) = (specs[0].id.clone(), specs[1].id.clone());
    let supervised = Supervised::start(
        specs,
        SupervisorConfig {
            probe_every: Duration::from_millis(50),
            ..SupervisorConfig::default()
        },
        Ports {
            host: ProcessHost::new().with_grace(Duration::from_millis(300)),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: FakeGpu(GpuMemory {
                total: MiB(16_000),
                used_by_others: MiB(0),
            }),
        },
    );
    assert_eq!(supervised.want(&first).await, Ok(()));
    let grandchild = pid_in(&files[0]).await;
    // The first engine is idle by now (a turn in the last probe window is never evicted).
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(supervised.want(&second).await, Ok(()));
    assert!(
        gone_soon(grandchild).await,
        "the evicted engine's grandchild is gone"
    );
}
