//! `ACCOUNTD_KEYS=file:<absolute path>` on the real binary and a private bus, with no Secret
//! Service anywhere: a `test-keys` build files credentials in the named 0600 file (`accountd add`
//! and the daemon share it), a shipped build ignores the variable and says so, and a test build
//! refuses a value or a file it cannot use before it serves. Nothing here touches the real
//! Secret Service or keyring; the environment is cleared.

mod common;

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

/// An accountd command in the scratch environment, with `ACCOUNTD_KEYS` as given.
fn accountd(bus: &PrivateBus, home: &Path, keys: Option<&str>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_accountd"));
    command
        .env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_RUNTIME_DIR", bus.scratch())
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address());
    if let Some(keys) = keys {
        command.env("ACCOUNTD_KEYS", keys);
    }
    command
}

fn spawn(bus: &PrivateBus, home: &Path, keys: Option<&str>, proc_root: Option<&Path>) -> Daemon {
    let stderr = home.join("stderr.log");
    let mut command = accountd(bus, home, keys);
    if let Some(root) = proc_root {
        command.env("ACCOUNTD_PROC_ROOT", root);
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
    for _ in 0..250 {
        if dbus
            .name_has_owner(porter_dbus::ACCOUNTS_BUS.try_into().expect("name"))
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
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    const KEY: &str = "sk-or-v1-S3CRET-SCRATCH-KEY-0123456789";

    /// Runs `accountd add`, typing `typed` on its standard input, to the end.
    fn add(
        bus: &PrivateBus,
        home: &Path,
        keys: &str,
        providers: &Path,
        typed: &str,
    ) -> (bool, String, String) {
        let mut child = accountd(bus, home, Some(keys))
            .arg("--providers")
            .arg(providers)
            .args(["add", "openrouter", "--allow", "org.example.Companion"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("accountd add runs");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(typed.as_bytes())
            .expect("typed");
        let output = child.wait_with_output().expect("add ends");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// The shipped OpenRouter provider file with its endpoint on a loopback fake of the company.
    fn provider_dir(home: &Path, endpoint: &str) -> PathBuf {
        let shipped =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../providers/openrouter.toml");
        let text = std::fs::read_to_string(shipped).expect("shipped file");
        let pointed = text.replace("https://openrouter.ai/api/v1", endpoint);
        assert_ne!(pointed, text, "the endpoint line was found");
        let dir = home.join("providers");
        std::fs::create_dir_all(&dir).expect("providers dir");
        std::fs::write(dir.join("openrouter.toml"), pointed).expect("provider file");
        dir
    }

    /// A /proc tree naming this process the Probe, a Flatpak app.
    fn proc_tree(dir: &Path) -> PathBuf {
        let root = dir.join("proc");
        let pid = root.join(std::process::id().to_string());
        std::fs::create_dir_all(&pid).expect("proc dir");
        std::fs::write(
            pid.join("cgroup"),
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.example.Probe-1.scope\n",
        )
        .expect("cgroup");
        root
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    /// `accountd add openrouter` into a key file, then the daemon over the same file: a porter
    /// daemon resolves the key on a sealed memfd, nothing on the bus carries it, and no Secret
    /// Service was involved (there is none on this bus, so any use of it would have failed).
    #[cfg(feature = "test-proc-root")]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_key_added_into_the_file_is_resolved_by_a_porter_daemon_with_no_secret_service() {
        use porter_fake_servers::{Auth, FakeLlmApi};
        use std::io::Read;

        let bus = PrivateBus::start();
        let home = scratch(&bus, "home");
        let keys = home.join("keys/keys.json");
        let keys_var = format!("file:{}", keys.display());
        let fake = FakeLlmApi::start(Auth::BearerKey)
            .await
            .expect("fake company");
        fake.seed_key(KEY);
        let providers = provider_dir(&home, &fake.api_url());

        // The terminal flow, key on standard input (echo is off only on a tty).
        let (ok, stdout, stderr) = add(&bus, &home, &keys_var, &providers, &format!("{KEY}\ny\n"));
        assert!(ok, "{stdout}\n{stderr}");
        assert!(stdout.contains("Added account openrouter"), "{stdout}");
        assert!(
            stderr.contains("TEST BUILD: credentials are in the file")
                && stderr.contains("keys.json"),
            "{stderr}"
        );
        assert!(!stdout.contains(KEY) && !stderr.contains(KEY));
        assert_eq!(mode(&keys), 0o600);
        let filed = std::fs::read_to_string(&keys).expect("the key file");
        assert!(
            filed.contains(KEY) && filed.contains("api_key"),
            "plain text, by design"
        );

        // The registry the add wrote names the account and the Companion's grant.
        let registry =
            std::fs::read_to_string(home.join("state/porter/registry.json")).expect("registry");
        let registry: serde_json::Value = serde_json::from_str(&registry).expect("json");
        let grant = registry["grants"][0]["id"]
            .as_str()
            .expect("a grant")
            .to_owned();

        // The daemon over the same file; the Probe is a porter daemon by the callers table.
        let config = home.join("config/porter");
        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::write(
            config.join("callers.toml"),
            "[[caller]]\napp = \"org.example.Probe\"\nrole = \"porter_daemon\"\n",
        )
        .expect("callers");
        let root = proc_tree(&home);
        let mut daemon = spawn(&bus, &home, Some(&keys_var), Some(&root));
        assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
        assert!(
            daemon
                .stderr()
                .contains("TEST BUILD: credentials are in the file"),
            "{}",
            daemon.stderr()
        );

        let mut tap = common::Tap::start(&bus).await;
        let client = bus.connect().await;
        let fd = porter_dbus::PeerProxy::new(&client)
            .await
            .expect("proxy")
            .resolve_key(&grant)
            .await
            .unwrap_or_else(|e| panic!("resolve: {e} {}", daemon.stderr()));
        let mut text = String::new();
        std::fs::File::from(std::os::fd::OwnedFd::from(fd))
            .read_to_string(&mut text)
            .expect("readable");
        assert_eq!(text, KEY);

        // The bus scan: no message of the whole exchange carries the key.
        let seen = tap.drain().await;
        assert!(!seen.is_empty(), "the tap saw the exchange");
        assert!(
            seen.iter().all(|message| !common::contains(message, KEY)),
            "the key is on the bus"
        );
        assert!(!daemon.stderr().contains(KEY));
        // Reading never loosened the file.
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
            let mut daemon = spawn(&bus, &home, Some(value), None);
            assert!(!serving(&bus, &mut daemon).await, "{value}");
            assert!(!daemon.child.wait().expect("exits").success(), "{value}");
            assert!(
                daemon.stderr().contains("ACCOUNTD_KEYS") && daemon.stderr().contains(says),
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
        let mut daemon = spawn(&bus, &home, Some(&var), None);
        assert!(!serving(&bus, &mut daemon).await);
        assert!(!daemon.child.wait().expect("exits").success());
        assert!(daemon.stderr().contains("0600"), "{}", daemon.stderr());
        assert_eq!(mode(&keys), 0o644, "not fixed behind the person's back");
        // `accountd add` refuses the same way, before it asks for anything.
        let providers = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../providers");
        let (ok, stdout, stderr) = add(&bus, &home, &var, &providers, "");
        assert!(
            !ok && !stdout.contains("Added") && stderr.contains("0600"),
            "{stdout}{stderr}"
        );
    }
}

/// A shipped build has no file store: it says it ignores the variable (never repeating it) and
/// keeps using the Secret Service, and creates no file.
#[cfg(not(feature = "test-keys"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_shipped_build_ignores_the_variable_and_says_so_without_repeating_it() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let keys = home.join("keys/keys.json");
    let var = format!("file:{}", keys.display());
    let mut daemon = spawn(&bus, &home, Some(&var), None);
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    let said = daemon.stderr();
    assert!(
        said.contains("ACCOUNTD_KEYS is set but this build has no test-keys feature; ignoring it and using the Secret Service"),
        "{said}"
    );
    assert_eq!(said.matches("ACCOUNTD_KEYS").count(), 1, "once: {said}");
    assert!(!said.contains("keys.json"));
    assert!(!keys.exists());
}
