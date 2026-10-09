//! `SIGTERM` and `SIGINT` stop inferd gracefully: the engines' process groups are ended, the exit
//! is 0, and a second signal (or the bound) kills everything at once.
//!
//! Two kinds of test. The real `inferd` binary on a private bus with a scratch XDG, no engines:
//! the signals are handled, the bus name is released, the exit is 0. And a stand-in for inferd
//! (this test binary run again, `fake_inferd`) that runs the same `inferd::shutdown` over a real
//! `ProcessHost` whose engine is a shell forking a grandchild that ignores `SIGTERM`: starting a
//! real engine needs a caller the real binary only names in a `test-proc-root` build, and no model
//! is ever run here. Only pids these tests started are signalled.

#[path = "it/hosting/bus.rs"]
mod bus;

use engine_supervisor::{EngineHost, EngineId, GpuAccess, Network, ProgramPath, Sandbox, UnitSpec};
use inferd::hosts::ProcessHost;
use inferd::shutdown::{Ended, Signals, on_signal};
use model_catalog::{EngineArg, MiB};
use porter_dbus::INFERENCE_BUS;
use rustix::process::{Pid, Signal, kill_process};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The stand-in's exit codes by how its shutdown ended.
const DONE: i32 = 0;
const TIMED_OUT: i32 = 4;
const HURRIED: i32 = 3;

/// The stand-in for inferd. Run as a test it does nothing; run by the tests below with
/// `FAKE_DIR`, `FAKE_GRACE_MS` and `FAKE_BOUND_MS` set, it starts the forking engine, says it is
/// ready, and waits for a signal to shut down as the daemon does.
#[test]
fn fake_inferd() {
    let Ok(dir) = std::env::var("FAKE_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let millis = |name: &str| {
        Duration::from_millis(
            std::env::var(name)
                .expect(name)
                .parse()
                .expect("milliseconds"),
        )
    };
    let (grace, bound) = (millis("FAKE_GRACE_MS"), millis("FAKE_BOUND_MS"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let ended = runtime.block_on(async {
        let mut signals = Signals::listen().expect("signals");
        let host = ProcessHost::new().with_grace(grace);
        let closer = host.closer();
        let d = dir.display();
        let script = format!(
            "(trap '' TERM; exec sleep 300) & echo $! > '{d}/grandchild'; echo $$ > '{d}/engine'; wait"
        );
        let unit = UnitSpec {
            program: ProgramPath(PathBuf::from("sh")),
            args: ["-c", script.as_str()]
                .iter()
                .map(|a| EngineArg((*a).into()))
                .collect(),
            env: vec![],
            sandbox: Sandbox {
                network: Network::None,
                read: vec![],
                write: vec![],
                gpu: GpuAccess::Absent,
                memory_max: MiB(0),
            },
        };
        host.spawn(&EngineId("e".into()), &unit)
            .await
            .expect("spawn");
        while !dir.join("grandchild").exists() || !dir.join("engine").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::write(dir.join("ready"), b"").expect("ready");
        let ended = on_signal(&mut signals, &closer, async {}, bound).await;
        // A host that is closed starts nothing more.
        let again = host.spawn(&EngineId("late".into()), &unit).await;
        assert!(again.is_err(), "a closed host started an engine");
        ended
    });
    std::process::exit(match ended {
        Ended::Done => DONE,
        Ended::TimedOut => TIMED_OUT,
        Ended::Hurried => HURRIED,
    });
}

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "sd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }
}

impl Drop for Scratch {
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

fn stand_in(dir: &Path, grace_ms: u32, bound_ms: u32) -> Started {
    Started(
        Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", "fake_inferd", "--nocapture"])
            .env("FAKE_DIR", dir)
            .env("FAKE_GRACE_MS", grace_ms.to_string())
            .env("FAKE_BOUND_MS", bound_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the stand-in"),
    )
}

fn pid_in(file: &Path) -> i32 {
    std::fs::read_to_string(file)
        .expect("pid file")
        .trim()
        .parse()
        .expect("a pid")
}

fn wait_for(file: &Path) {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if file.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    deadline.fail(&format!("{} to appear", file.display()));
}

/// Gone: no such process, or a zombie nobody has collected yet.
fn gone(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

fn gone_soon(pid: i32) -> bool {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if gone(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn signal(pid: u32, signal: Signal) {
    let pid = Pid::from_raw(i32::try_from(pid).expect("pid")).expect("a pid");
    kill_process(pid, signal).expect("signal a process this test started");
}

/// The exit code of `child` within `within`, or `None` (then the test fails; the guard kills it).
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

/// Starts the stand-in and returns it with its engine's and grandchild's pids, both alive.
fn running(dir: &Scratch, grace_ms: u32, bound_ms: u32) -> (Started, i32, i32) {
    let inferd = stand_in(&dir.0, grace_ms, bound_ms);
    wait_for(&dir.0.join("ready"));
    let (engine, grandchild) = (
        pid_in(&dir.0.join("engine")),
        pid_in(&dir.0.join("grandchild")),
    );
    assert!(!gone(engine) && !gone(grandchild));
    (inferd, engine, grandchild)
}

/// Ends what a failed test left running.
fn reap(pids: &[i32]) {
    for pid in pids {
        if let Some(pid) = Pid::from_raw(*pid) {
            let _ = kill_process(pid, Signal::KILL);
        }
    }
}

fn graceful(signal_sent: Signal) {
    let dir = Scratch::new();
    // A grace of 300 ms: the grandchild ignores SIGTERM, so the stop ends with its SIGKILL.
    let (mut inferd, engine, grandchild) = running(&dir, 300, 5000);
    let started = Instant::now();
    signal(inferd.0.id(), signal_sent);
    let code = exit_within(&mut inferd, Duration::from_secs(8));
    let both = gone_soon(engine) && gone_soon(grandchild);
    if !both {
        reap(&[engine, grandchild]);
    }
    assert_eq!(code, Some(DONE), "exit within the bound");
    assert!(both, "the engine and its grandchild are gone");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn sigterm_ends_the_engine_group_with_its_grandchild_and_exits_zero() {
    graceful(Signal::TERM);
}

#[test]
fn sigint_ends_the_engine_group_with_its_grandchild_and_exits_zero() {
    graceful(Signal::INT);
}

#[test]
fn a_second_signal_during_shutdown_kills_everything_and_exits_at_once() {
    let dir = Scratch::new();
    // A grace of 60 s: the first signal's shutdown is still waiting on the grandchild.
    let (mut inferd, engine, grandchild) = running(&dir, 60_000, 60_000);
    signal(inferd.0.id(), Signal::TERM);
    std::thread::sleep(Duration::from_millis(600));
    let waiting = inferd.0.try_wait().expect("wait").is_none() && !gone(grandchild);
    let second = Instant::now();
    if waiting {
        signal(inferd.0.id(), Signal::TERM);
    }
    let code = exit_within(&mut inferd, Duration::from_secs(5));
    let both = gone_soon(engine) && gone_soon(grandchild);
    if !both {
        reap(&[engine, grandchild]);
    }
    assert!(
        waiting,
        "the shutdown was still running before the second signal"
    );
    assert_eq!(code, Some(HURRIED));
    assert!(second.elapsed() < Duration::from_secs(4));
    assert!(both, "the engine and its grandchild are gone");
}

#[test]
fn a_shutdown_that_overruns_its_bound_kills_everything_and_exits_nonzero() {
    let dir = Scratch::new();
    let (mut inferd, engine, grandchild) = running(&dir, 60_000, 700);
    signal(inferd.0.id(), Signal::TERM);
    let code = exit_within(&mut inferd, Duration::from_secs(5));
    let both = gone_soon(engine) && gone_soon(grandchild);
    if !both {
        reap(&[engine, grandchild]);
    }
    assert_eq!(code, Some(TIMED_OUT));
    assert!(both, "the engine and its grandchild are gone");
}

/// The real inferd on a private bus, no engines, nothing probed: the name is held, then released.
fn real_inferd(bus: &bus::PrivateBus) -> Started {
    let scratch = bus.scratch();
    let config = scratch.join("inferd.toml");
    std::fs::write(
        &config,
        "[probe]\nollama = []\nllama_cpp = []\nlm_studio = []\n",
    )
    .expect("config");
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
            .arg(&config)
            .stderr(Stdio::null())
            .spawn()
            .expect("inferd binary"),
    )
}

async fn real_binary_stops_on(signal_sent: Signal) {
    let bus = bus::PrivateBus::start();
    let client = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&client).await.expect("dbus");
    let name = zbus::names::BusName::try_from(INFERENCE_BUS).expect("bus name");
    let mut inferd = real_inferd(&bus);
    let until = Instant::now() + Duration::from_secs(20);
    while !dbus.name_has_owner(name.clone()).await.expect("ask") {
        assert!(Instant::now() < until, "inferd never took its name");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    signal(inferd.0.id(), signal_sent);
    let code = exit_within(&mut inferd, Duration::from_secs(15));
    assert_eq!(code, Some(0), "a graceful stop is exit 0");
    assert!(
        !dbus.name_has_owner(name).await.expect("ask"),
        "the bus name is released"
    );
}

#[tokio::test]
async fn the_real_inferd_stops_on_sigterm_with_exit_zero_and_releases_its_name() {
    real_binary_stops_on(Signal::TERM).await;
}

#[tokio::test]
async fn the_real_inferd_stops_on_sigint_with_exit_zero_and_releases_its_name() {
    real_binary_stops_on(Signal::INT).await;
}
