//! A private session bus for one test: its own `dbus-daemon` on an abstract socket, with a
//! scratch HOME and runtime directory and no environment of the real session. Dropping it
//! stops the daemon by its PID. Tests never reach the real bus.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use test_kit::Scratch;

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

/// A running private bus.
#[derive(Debug)]
pub struct PrivateBus {
    child: Child,
    address: String,
    scratch: Scratch,
}

impl PrivateBus {
    /// Starts the daemon and waits for its address.
    pub fn start() -> Self {
        let scratch = Scratch::new("bus");
        let config = scratch.join("bus.conf");
        std::fs::write(&config, config_text(&scratch)).expect("write the bus config");
        let mut child = Command::new("dbus-daemon")
            .env_clear()
            .env("HOME", scratch.path())
            .env("XDG_RUNTIME_DIR", scratch.path())
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
    pub fn scratch(&self) -> &Path {
        self.scratch.path()
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
    }
}
