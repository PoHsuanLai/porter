//! What the rig's integration tests share: a private bus, a scratch directory, a daemon process
//! that is stopped by its PID. Nothing here reaches the real session, `~/.config` or `/etc`.
#![allow(dead_code)]

pub mod bus;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// A scratch directory under the build's own tmp dir.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rig-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
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

/// Polls `check` every 20 ms for up to thirty seconds.
pub async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    for _ in 0..1500 {
        if let Some(found) = check() {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never happened: {what}");
}
