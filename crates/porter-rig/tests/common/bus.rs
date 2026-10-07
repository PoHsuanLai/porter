//! A private session bus for one test: its own `dbus-daemon` on an abstract socket, with a
//! scratch HOME and runtime directory and no environment of the real session. Dropping it
//! stops the daemon by its PID. Tests never reach the real bus.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

fn config_text(dir: &std::path::Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        dir.display()
    )
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A running private bus.
#[derive(Debug)]
pub struct PrivateBus {
    child: Child,
    address: String,
    scratch: PathBuf,
}

impl PrivateBus {
    /// Starts the daemon and waits for its address.
    pub fn start() -> Self {
        let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "bus-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let config = scratch.join("bus.conf");
        std::fs::write(&config, config_text(&scratch)).expect("write the bus config");
        let mut child = Command::new("dbus-daemon")
            .env_clear()
            .env("HOME", &scratch)
            .env("XDG_RUNTIME_DIR", &scratch)
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon must be on PATH for the bus tests");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut address = String::new();
        BufReader::new(stdout)
            .read_line(&mut address)
            .expect("the daemon prints its address");
        Self {
            child,
            address: address.trim().to_owned(),
            scratch,
        }
    }

    /// The bus address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The scratch directory (HOME and runtime dir of anything the test runs).
    pub fn scratch(&self) -> &PathBuf {
        &self.scratch
    }

    /// A new connection to this bus.
    pub async fn connect(&self) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .expect("address")
            .build()
            .await
            .expect("connect to the private bus")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}
