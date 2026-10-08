//! A private session bus for one test: its own `dbus-daemon` on an abstract socket, with a
//! scratch HOME and runtime directory and no environment of the real session. Dropping it
//! stops the daemon. Tests never reach the real bus.
//!
//! Every wait of a test has a deadline ([`within`], [`DEADLINE`]): a wait that does not end is a
//! failure that names what it waited for, not a hang until the harness ends the test.

use std::future::Future;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// How long any one wait of a test may take. A wait ends in milliseconds, in seconds on a loaded
/// machine; the harness ends a whole test after ten minutes.
pub const DEADLINE: Duration = Duration::from_secs(60);

/// What `future` gives, or a panic naming `what` once [`DEADLINE`] has passed.
pub async fn within<T>(what: &str, future: impl Future<Output = T>) -> T {
    match tokio::time::timeout(DEADLINE, future).await {
        Ok(out) => out,
        Err(_) => panic!("waited {} s for {what}", DEADLINE.as_secs()),
    }
}

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
        // The read blocks, so it runs on a thread of its own and the wait for it is bounded.
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut address = String::new();
            let read = BufReader::new(stdout).read_line(&mut address);
            let _ = said.send(read.map(|_| address));
        });
        let address = match heard.recv_timeout(DEADLINE) {
            Ok(read) => read.expect("the daemon prints its address"),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "waited {} s for dbus-daemon to print its address",
                    DEADLINE.as_secs()
                );
            }
        };
        assert!(
            !address.trim().is_empty(),
            "dbus-daemon ended without printing its address"
        );
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

    /// A new connection to this bus. A method call on it that is not answered within
    /// [`DEADLINE`] is an error (`TimedOut`), not a wait without end.
    pub async fn connect(&self) -> zbus::Connection {
        let builder = zbus::connection::Builder::address(self.address.as_str())
            .expect("address")
            .method_timeout(DEADLINE);
        within("a connection to the private bus", builder.build())
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
