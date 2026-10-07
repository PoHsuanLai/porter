//! What becomes of an engine's processes when inferd itself ends. A helper process stands in for
//! inferd: this test binary run again (`fake_inferd`), starting an engine through the real
//! `ProcessHost`. The engine is a shell that forks a grandchild ignoring `SIGTERM` and records
//! both pids in a scratch directory. Only pids this test started are signalled.

use engine_supervisor::{EngineHost, EngineId, GpuAccess, Network, ProgramPath, Sandbox, UnitSpec};
use inferd::hosts::ProcessHost;
use model_catalog::{EngineArg, MiB};
use rustix::process::{Pid, Signal, kill_process};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// The stand-in for inferd. Run as a test it does nothing; run by the tests below, with
/// `FAKE_INFERD` set, it starts the forking engine, says it is ready, and then either waits to be
/// killed (`linger`) or drops the host and returns (`exit_normally`).
#[test]
fn fake_inferd() {
    let Ok(mode) = std::env::var("FAKE_INFERD") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("FAKE_DIR").expect("FAKE_DIR"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let host = ProcessHost::new();
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
        match mode.as_str() {
            "linger" => tokio::time::sleep(Duration::from_secs(60)).await,
            "exit_normally" => drop(host),
            other => panic!("no mode {other}"),
        }
    });
}

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "eg-{}-{}",
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

fn helper(mode: &str, dir: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", "fake_inferd", "--nocapture"])
        .env("FAKE_INFERD", mode)
        .env("FAKE_DIR", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the stand-in")
}

fn pid_in(file: &Path) -> i32 {
    std::fs::read_to_string(file)
        .expect("pid file")
        .trim()
        .parse()
        .expect("a pid")
}

fn wait_for(file: &Path) {
    for _ in 0..400 {
        if file.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("no {}", file.display());
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
    for _ in 0..200 {
        if gone(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn kill_pid(pid: i32) {
    if let Some(pid) = Pid::from_raw(pid) {
        let _ = kill_process(pid, Signal::KILL);
    }
}

#[test]
fn a_killed_inferd_takes_its_direct_engine_with_it_and_leaves_the_grandchild_to_the_unit() {
    let dir = Scratch::new();
    let mut inferd = helper("linger", &dir.0);
    wait_for(&dir.0.join("ready"));
    let (engine, grandchild) = (
        pid_in(&dir.0.join("engine")),
        pid_in(&dir.0.join("grandchild")),
    );
    assert!(!gone(engine) && !gone(grandchild));

    inferd.kill().expect("SIGKILL the stand-in");
    inferd.wait().expect("reaped");

    // PR_SET_PDEATHSIG: the engine is killed with its parent.
    assert!(
        gone_soon(engine),
        "the direct child outlived a SIGKILLed inferd"
    );
    // It reaches no grandchild: that is the unit's cgroup's to end (KillMode=control-group). Outside
    // a unit it lives on; this test ends it.
    assert!(!gone(grandchild));
    kill_pid(grandchild);
    assert!(gone_soon(grandchild));
}

#[test]
fn an_inferd_that_exits_normally_leaves_no_engine_process() {
    let dir = Scratch::new();
    let mut inferd = helper("exit_normally", &dir.0);
    wait_for(&dir.0.join("ready"));
    let (engine, grandchild) = (
        pid_in(&dir.0.join("engine")),
        pid_in(&dir.0.join("grandchild")),
    );
    let status = inferd.wait().expect("the stand-in ends");
    assert!(status.success(), "{status:?}");
    let both = gone_soon(engine) && gone_soon(grandchild);
    if !both {
        kill_pid(engine);
        kill_pid(grandchild);
    }
    assert!(both, "the engine and its grandchild are gone");
}
