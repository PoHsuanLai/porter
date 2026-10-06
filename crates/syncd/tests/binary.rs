//! The syncd binary on a private bus with scratch HOME and XDG: it serves `Sync1` with no dataset,
//! finds callers through a /proc root only in a `test-proc-root` build, wipes an account when
//! accountd says it is gone, and refuses to start over a caller table it cannot read.

mod common;

use common::bus::PrivateBus;
use common::error_name;
use porter_dbus::{SYNC_BUS, SyncProxy};
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_syncd"));
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
        command.env("SYNCD_PROC_ROOT", root);
    }
    Daemon {
        child: command.spawn().expect("syncd starts"),
        stderr,
    }
}

async fn serving(bus: &PrivateBus, daemon: &mut Daemon) -> bool {
    let probe = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&probe).await.expect("proxy");
    for _ in 0..250 {
        if dbus
            .name_has_owner(SYNC_BUS.try_into().expect("name"))
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

#[cfg(feature = "test-proc-root")]
#[tokio::test(flavor = "multi_thread")]
async fn a_test_build_reads_callers_from_the_proc_root_and_serves_an_empty_sync1() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let root = proc_tree(&home);
    let mut daemon = spawn(&bus, &home, Some(&root));
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    assert!(
        daemon.stderr().contains("reading callers from") && daemon.stderr().contains("proc"),
        "{}",
        daemon.stderr()
    );
    let client = bus.connect().await;
    let sync = SyncProxy::new(&client).await.expect("proxy");
    assert!(sync.datasets().await.expect("datasets").is_empty());
    let err = sync
        .status("a1/pim")
        .await
        .map(drop)
        .expect_err("no dataset yet");
    assert_eq!(error_name(&err), common::NO_FITTING);
}

#[cfg(feature = "test-proc-root")]
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_wipes_an_account_when_accountd_says_it_is_gone() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
    let journal = home.join("state/porter/sync/a1");
    let mirror = home.join("data/porter/vdir/a1/personal");
    let other = home.join("state/porter/sync/a2");
    for dir in [&journal, &mirror, &other] {
        std::fs::create_dir_all(dir).expect("dir");
        std::fs::write(dir.join("x"), "data").expect("file");
    }
    let mut daemon = spawn(&bus, &home, None);
    assert!(serving(&bus, &mut daemon).await, "{}", daemon.stderr());
    // This connection plays accountd; the daemon found it by name and told it who it is.
    let accountd = bus.connect().await;
    accountd
        .request_name(porter_dbus::ACCOUNTS_BUS)
        .await
        .expect("name");
    let emit = |account: &'static str| {
        let accountd = accountd.clone();
        async move {
            let path = zbus::zvariant::ObjectPath::try_from(format!(
                "{}/account/{account}",
                porter_dbus::ACCOUNTS_PATH
            ))
            .expect("path");
            accountd
                .emit_signal(
                    None::<zbus::names::BusName<'_>>,
                    porter_dbus::ACCOUNTS_PATH,
                    "org.quire.Accounts1.Manager",
                    "AccountRemoved",
                    &(path,),
                )
                .await
                .expect("emit");
        }
    };
    // The daemon listens from its start; broadcast until it has acted (the match rule may be
    // added a moment after the name appears).
    for _ in 0..250 {
        emit("a1").await;
        if !journal.exists() && !home.join("data/porter/vdir/a1").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    common::eventually("a1 gone", || {
        !journal.exists() && !home.join("data/porter/vdir/a1").exists()
    })
    .await;
    assert!(other.exists(), "a2 is untouched");
}

#[cfg(not(feature = "test-proc-root"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_release_build_ignores_the_proc_root_variable() {
    let bus = PrivateBus::start();
    let home = scratch(&bus, "home");
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
    let err = SyncProxy::new(&client)
        .await
        .expect("proxy")
        .datasets()
        .await
        .expect_err("the fixture was not read");
    assert_eq!(error_name(&err), common::ACCESS_DENIED);
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
