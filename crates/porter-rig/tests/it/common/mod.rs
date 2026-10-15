//! What the rig's integration tests share: a private bus, a scratch directory, a daemon process
//! that is stopped by its PID. Nothing here reaches the real session, `~/.config` or `/etc`.
#![allow(dead_code)]

pub mod bus;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A scratch directory under the build's own tmp dir, its own on every call.
///
/// The tests of one binary run in parallel in one process, so the process id alone does not tell
/// two of them apart: two rigs named alike shared one directory, and each emptied it as it
/// started. The client tests then saw another test's live fixtures in their proc root, or lost
/// their own to the other's start. A count makes every call's directory its own.
pub fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let call = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = porter_fake::scratch_dir(&format!("rig-{call}-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// A child process, killed by its PID when the guard goes.
#[derive(Debug)]
pub struct Proc {
    pub child: Child,
}

impl Proc {
    /// Spawns `program` with a clean environment plus `envs`.
    pub fn spawn(program: &str, args: &[&str], envs: &[(&str, &Path)]) -> Self {
        let mut command = Command::new(program);
        command
            .env_clear()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (name, value) in envs {
            command.env(name, value);
        }
        Self {
            child: command.spawn().expect("the rig binary starts"),
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Sends SIGTERM to this PID and waits for the process to end; its exit status.
    pub fn terminate(&mut self) -> std::process::ExitStatus {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.child.id()).expect("pid"))
            .expect("a pid");
        rustix::process::kill_process(pid, rustix::process::Signal::TERM).expect("SIGTERM");
        self.child.wait().expect("the process ends")
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls `check` every 20 ms until `porter_fake::GENEROUS` has passed by the clock.
pub async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if let Some(found) = check() {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}
