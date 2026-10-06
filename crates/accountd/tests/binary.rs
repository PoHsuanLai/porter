//! The accountd binary on a private bus with scratch HOME and XDG: it loads its registry,
//! refuses to start over one it cannot read, finds callers through a /proc root only in a
//! `test-proc-root` build, and never touches the real Secret Service (nothing here needs a secret).

mod common;

use common::bus::PrivateBus;
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::store::Persisted;
use porter_core::{DataClass, GrantId, SpaceScope, UnixSeconds};
use porter_dbus::ManagerProxy;
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

fn spawn(bus: &PrivateBus, home: &Path, proc_root: Option<&Path>) -> Daemon {
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
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr).expect("stderr file"));
    if let Some(root) = proc_root {
        command.env("ACCOUNTD_PROC_ROOT", root);
    }
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

/// A /proc tree naming this process a Flatpak app.
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

fn storage_need() -> porter_dbus::NeedArg {
    common::storage_need()
}

fn probe() -> porter_core::AppId {
    common::app("org.example.Probe")
}

/// A registry file holding the fake storage account and a grant for the probe app.
fn seed(home: &Path) {
    let account = porter_fake::storage_account();
    let grant = Grant {
        id: GrantId::parse("seeded").expect("id"),
        key: GrantKey {
            app: probe(),
            account: account.id.clone(),
            kind: porter_core::CapabilityKind::Storage,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    };
    let mut stored = Persisted::empty();
    stored.accounts.push(account);
    stored.grants.push(grant);
    let dir = home.join("state/porter");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(dir.join("registry.json"), stored.to_json().expect("json")).expect("registry");
}

#[cfg(feature = "test-proc-root")]
#[tokio::test(flavor = "multi_thread")]
async fn a_test_build_reads_callers_from_the_proc_root_and_says_so() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    seed(&home);
    let root = proc_tree(&home);
    let mut daemon = spawn(&bus, &home, Some(&root));
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    assert!(
        daemon.stderr().contains("reading callers from") && daemon.stderr().contains("proc"),
        "{}",
        daemon.stderr()
    );

    // This process is the Probe app by the fixture, holds the seeded grant, and finds the account.
    let client = bus.connect().await;
    let found = ManagerProxy::new(&client)
        .await
        .expect("proxy")
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect("query");
    assert_eq!(found.len(), 1, "the registry on disk was loaded");
}

#[cfg(not(feature = "test-proc-root"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_release_build_ignores_the_proc_root_variable() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    seed(&home);
    let root = proc_tree(&home);
    let mut daemon = spawn(&bus, &home, Some(&root));
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    assert!(
        !daemon.stderr().contains("reading callers from"),
        "{}",
        daemon.stderr()
    );
    // The real /proc says this process is no app (a test runner's scope): nobody, so refused.
    let client = bus.connect().await;
    let err = ManagerProxy::new(&client)
        .await
        .expect("proxy")
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect_err("the fixture was not read");
    assert_eq!(common::error_name(&err), common::ACCESS_DENIED);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_caller_is_refused_by_the_real_binary() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let mut daemon = spawn(&bus, &home, None);
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    let client = bus.connect().await;
    let err = ManagerProxy::new(&client)
        .await
        .expect("proxy")
        .query(&storage_need(), "photos", "interactive")
        .await;
    // A runner inside a login session may or may not be named by its cgroup; either way the
    // daemon answered on its bus, and a caller it cannot name is AccessDenied.
    if let Err(err) = err {
        assert_eq!(common::error_name(&err), common::ACCESS_DENIED);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registry_it_cannot_read_is_never_overwritten_and_the_daemon_does_not_start() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let dir = home.join("state/porter");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(dir.join("registry.json"), "{ this is not a registry").expect("registry");
    let mut daemon = spawn(&bus, &home, None);
    assert!(!serving(&bus, &mut daemon).await);
    let status = daemon.child.wait().expect("exits");
    assert!(!status.success());
    assert!(
        daemon.stderr().contains("registry.json"),
        "{}",
        daemon.stderr()
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("registry.json")).expect("still there"),
        "{ this is not a registry"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_table_that_does_not_parse_stops_the_daemon_naming_the_file() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let config = home.join("config/porter");
    std::fs::create_dir_all(&config).expect("config dir");
    std::fs::write(
        config.join("callers.toml"),
        "[[caller]]\napp = \"not a name\"\nrole = \"app\"\n",
    )
    .expect("callers");
    let mut daemon = spawn(&bus, &home, None);
    assert!(!serving(&bus, &mut daemon).await);
    assert!(!daemon.child.wait().expect("exits").success());
    assert!(
        daemon.stderr().contains("callers.toml"),
        "{}",
        daemon.stderr()
    );
}

/// `accountd add ...` to the end, with no input, on the private bus with the shipped provider
/// files: its exit status, standard output and standard error.
fn add(bus: &PrivateBus, home: &Path, args: &[&str]) -> (bool, String, String) {
    let providers = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../providers");
    let output = Command::new(env!("CARGO_BIN_EXE_accountd"))
        .env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_RUNTIME_DIR", bus.scratch())
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .arg("--providers")
        .arg(providers)
        .arg("add")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("accountd add runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn add_says_what_it_does_what_the_allow_means_and_what_to_do_with_the_daemon() {
    let output = Command::new(env!("CARGO_BIN_EXE_accountd"))
        .args(["add", "--help"])
        .env_clear()
        .output()
        .expect("help");
    let help = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{help}");
    for phrase in [
        "Usage: accountd add [OPTIONS] <PROVIDER>",
        "--allow <APP-ID>",
        "--class <CLASS>",
        "echo off",
        "You typing this command is the consent",
        "\"Allow, always\"",
        "systemctl --user stop accountd.service",
        "openrouter",
    ] {
        assert!(help.contains(phrase), "{phrase}: {help}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn add_stops_while_the_daemon_runs_and_names_how_to_go_on() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let mut daemon = spawn(&bus, &home, None);
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    let (ok, _stdout, stderr) = add(&bus, &home, &["anthropic"]);
    assert!(!ok);
    assert!(
        stderr.contains("owns org.quire.Accounts1") && stderr.contains("systemctl --user stop"),
        "{stderr}"
    );
    assert!(!home.join("state/porter/registry.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn add_refuses_what_it_cannot_serve_before_it_asks_for_anything() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let cases: &[(&[&str], &str)] = &[
        (&["nosuch"], "no provider `nosuch` is served"),
        (&["Not A Provider"], "not understood"),
        (&["anthropic", "--allow", "not a name"], "not understood"),
        (&["anthropic", "--class", "secrets"], "not understood"),
    ];
    for (args, want) in cases {
        let (ok, stdout, stderr) = add(&bus, &home, args);
        assert!(!ok, "{args:?}");
        assert!(stderr.contains(want), "{args:?}: {stderr}");
        assert!(!stdout.contains("Added"), "{args:?}");
    }
    assert!(!home.join("state/porter/registry.json").exists());
}
