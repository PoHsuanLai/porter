//! `ACCOUNTD_KEYS=file:<absolute path>` on the real binary and a private bus, with no Secret
//! Service anywhere: a `test-keys` build refuses a value or a file it cannot use before it
//! serves, and a shipped build ignores the variable and says so. Nothing here touches the real
//! Secret Service or keyring; the environment is cleared.

use crate::common;

use common::bus::PrivateBus;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The daemon process, killed by its PID when the guard goes.
struct Daemon {
    child: Child,
    stderr: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Daemon {
    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.stderr).unwrap_or_default()
    }
}

fn scratch(bus: &PrivateBus, name: &str) -> PathBuf {
    let dir = bus.scratch().join(name);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn spawn(bus: &PrivateBus, home: &Path, keys: Option<&str>) -> Daemon {
    let stderr = home.join("stderr.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_accountd"));
    command
        .env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_RUNTIME_DIR", bus.scratch())
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        // Never the real Tailscale: a socket in the scratch directory that nobody serves.
        .env(
            accountd::paths::TAILSCALE_SOCKET_VAR,
            home.join("no-tailscale.sock"),
        );
    if let Some(keys) = keys {
        command.env("ACCOUNTD_KEYS", keys);
    }
    command
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr).expect("stderr file"));
    Daemon {
        child: command.spawn().expect("accountd starts"),
        stderr,
    }
}

async fn serving(bus: &PrivateBus, daemon: &mut Daemon) -> bool {
    let probe = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&probe).await.expect("proxy");
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if dbus
            .name_has_owner(porter_dbus::names::ACCOUNTS_BUS.try_into().expect("name"))
            .await
            .unwrap_or(false)
        {
            return true;
        }
        if daemon.child.try_wait().ok().flatten().is_some() {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

#[cfg(feature = "test-keys")]
mod test_build {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_test_build_says_where_credentials_are_and_makes_the_key_file_private() {
        let bus = PrivateBus::start();
        let home = scratch(&bus, "home");
        let keys = home.join("keys/keys.json");
        let var = format!("file:{}", keys.display());
        let mut daemon = spawn(&bus, &home, Some(&var));
        assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
        assert!(
            daemon
                .stderr()
                .contains("TEST BUILD: credentials are in the file")
                && daemon.stderr().contains("keys.json"),
            "{}",
            daemon.stderr()
        );
        assert_eq!(mode(&keys), 0o600);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_value_that_is_not_an_absolute_file_path_stops_the_daemon_before_it_serves() {
        for (value, says) in [
            ("memory", "is not file:<absolute path>"),
            ("file:", "is not file:<absolute path>"),
            ("", "is not file:<absolute path>"),
            ("file:relative/keys.json", "is not absolute"),
        ] {
            let bus = PrivateBus::start();
            let home = scratch(&bus, "home");
            let mut daemon = spawn(&bus, &home, Some(value));
            assert!(!serving(&bus, &mut daemon).await, "{value}");
            assert!(!daemon.child.wait().expect("exits").success(), "{value}");
            assert!(
                daemon.stderr().contains(says),
                "{value}: {}",
                daemon.stderr()
            );
            assert!(!home.join("relative").exists());
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_key_file_open_to_group_or_other_stops_the_daemon_and_is_left_alone() {
        let bus = PrivateBus::start();
        let home = scratch(&bus, "home");
        let keys = home.join("keys.json");
        std::fs::write(&keys, r#"{"version":1,"items":[]}"#).expect("file");
        std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let var = format!("file:{}", keys.display());
        let mut daemon = spawn(&bus, &home, Some(&var));
        assert!(!serving(&bus, &mut daemon).await);
        assert!(!daemon.child.wait().expect("exits").success());
        assert!(daemon.stderr().contains("0600"), "{}", daemon.stderr());
        assert_eq!(mode(&keys), 0o644, "not fixed behind the person's back");
    }
}

/// A shipped build has no file store: it says it ignores the setting (never repeating the
/// variable's value) and keeps using the Secret Service, and creates no file.
#[cfg(not(feature = "test-keys"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_shipped_build_ignores_the_variable_and_says_so_without_repeating_it() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let keys = home.join("keys/keys.json");
    let var = format!("file:{}", keys.display());
    let mut daemon = spawn(&bus, &home, Some(&var));
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    let said = daemon.stderr();
    assert!(
        said.contains(
            "this build has no test-keys feature; ignoring it and using the Secret Service"
        ),
        "{said}"
    );
    assert!(!said.contains("keys.json"));
    assert!(!keys.exists());
}
