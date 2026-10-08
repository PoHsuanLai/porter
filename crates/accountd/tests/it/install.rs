//! `scripts/install.sh`: run into a staging directory (`--destdir`) with stand-in daemons, it puts
//! every file the units and the activation files name where they say, again changes nothing, keeps a
//! caller table the person edited, and writes nothing under a home directory.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A fresh scratch directory under the test temp dir (`TMPDIR`), named for the test and process.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("porter-install-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// Stand-in daemons: the script installs what it is given, it does not need real builds.
fn stand_ins(root: &Path) -> PathBuf {
    let bins = root.join("bins");
    fs::create_dir_all(&bins).expect("bins");
    for daemon in ["accountd", "syncd", "inferd"] {
        let file = bins.join(daemon);
        fs::write(&file, format!("#!/bin/sh\necho {daemon}\n")).expect("write");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    bins
}

struct Run {
    destdir: PathBuf,
    home: PathBuf,
    bins: PathBuf,
}

impl Run {
    fn new(name: &str) -> Run {
        let root = scratch(name);
        let home = root.join("home");
        fs::create_dir_all(&home).expect("home");
        Run {
            destdir: root.join("stage"),
            bins: stand_ins(&root),
            home,
        }
    }

    /// The script's standard output.
    fn install(&self, extra: &[&str]) -> String {
        let out = Command::new("sh")
            .arg(repo().join("scripts/install.sh"))
            .args(["--bin-dir"])
            .arg(&self.bins)
            .arg("--destdir")
            .arg(&self.destdir)
            .args(extra)
            .env("HOME", &self.home)
            .env_remove("PREFIX")
            .env_remove("DESTDIR")
            .output()
            .expect("run the script");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf-8")
    }

    fn at(&self, path: &str) -> PathBuf {
        self.destdir.join(path.trim_start_matches('/'))
    }
}

fn files_under(dir: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.insert(path);
            }
        }
    }
    found
}

fn value(file: &str, key: &str) -> Option<String> {
    file.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(str::to_owned)
}

/// The executable of an `ExecStart=` line: systemd's `-`, `+`, `!` and `@` prefixes dropped.
fn program(line: &str) -> &str {
    let line = line.trim_start_matches(['-', '+', '!', '@', ':']);
    line.split_whitespace().next().unwrap_or("")
}

#[test]
fn every_file_a_unit_or_an_activation_file_names_is_installed_at_the_path_it_names() {
    for prefix in ["/usr", "/opt/porter-try"] {
        let run = Run::new(&format!("names{}", prefix.replace('/', "-")));
        run.install(&["--prefix", prefix]);
        let units = run.at(&format!("{prefix}/lib/systemd/user"));
        let mut checked = 0;
        for unit in ["accountd", "syncd", "inferd"] {
            let text = fs::read_to_string(units.join(format!("{unit}.service"))).expect("unit");
            let exec = value(&text, "ExecStart").expect("ExecStart");
            assert_eq!(program(&exec), format!("{prefix}/libexec/quire/{unit}"));
            assert!(run.at(program(&exec)).is_file(), "{exec}");
            checked += 1;
        }
        let activations = files_under(&run.at(&format!("{prefix}/share/dbus-1/services")));
        assert_eq!(activations.len(), 3);
        for path in activations {
            let text = fs::read_to_string(&path).expect("activation");
            let exec = value(&text, "Exec").expect("Exec");
            assert!(run.at(&exec).is_file(), "{} names {exec}", path.display());
            let service = value(&text, "SystemdService").expect("SystemdService");
            let unit = fs::read_to_string(units.join(&service)).expect("its unit is installed");
            assert_eq!(value(&unit, "BusName"), value(&text, "Name"));
            assert_eq!(value(&unit, "ExecStart"), Some(exec));
            checked += 1;
        }
        assert_eq!(checked, 6);
    }
}

#[test]
fn the_data_the_daemons_read_lands_where_they_look_for_it() {
    let run = Run::new("data");
    run.install(&[]);
    // accountd reads /usr/share/porter/providers and /etc/porter/callers.toml (paths.rs).
    let shipped: BTreeSet<String> = files_under(&repo().join("providers"))
        .iter()
        .map(|p| p.file_name().expect("name").to_string_lossy().into_owned())
        .collect();
    let installed: BTreeSet<String> = files_under(&run.at("/usr/share/porter/providers"))
        .iter()
        .map(|p| p.file_name().expect("name").to_string_lossy().into_owned())
        .collect();
    assert!(!shipped.is_empty());
    assert_eq!(installed, shipped);
    for path in [
        "/etc/porter/callers.toml",
        "/usr/share/quire/settings/inferd.settings.toml",
        "/usr/share/doc/porter/examples/inferd.toml",
        "/usr/share/doc/porter/examples/inferd-cloud.conf",
    ] {
        assert!(run.at(path).is_file(), "{path}");
    }
    // No clients file is shipped until the owner has one to ship.
    assert_eq!(
        run.at("/usr/share/porter/clients.toml").exists(),
        repo().join("dist/clients.toml").exists()
    );
    // The daemons are executable, the data is not.
    let mode = |path: &str| {
        fs::metadata(run.at(path))
            .expect("meta")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode("/usr/libexec/quire/accountd"), 0o755);
    assert_eq!(mode("/etc/porter/callers.toml"), 0o644);
}

#[test]
fn a_second_run_changes_nothing_and_a_home_directory_is_never_written() {
    let run = Run::new("again");
    let first = run.install(&[]);
    assert!(
        first.contains("installed /usr/libexec/quire/accountd"),
        "{first}"
    );
    let before: Vec<_> = files_under(&run.destdir)
        .into_iter()
        .map(|p| {
            let modified = fs::metadata(&p).expect("meta").modified().expect("mtime");
            (p, modified)
        })
        .collect();
    let second = run.install(&[]);
    assert_eq!(second.trim(), "nothing to change");
    let after: Vec<_> = files_under(&run.destdir)
        .into_iter()
        .map(|p| {
            let modified = fs::metadata(&p).expect("meta").modified().expect("mtime");
            (p, modified)
        })
        .collect();
    assert_eq!(before, after);
    assert!(files_under(&run.home).is_empty());
    // Everything is under the prefix or the configuration directory, and nothing is left staged.
    for path in files_under(&run.destdir) {
        let rel = path.strip_prefix(&run.destdir).expect("under destdir");
        assert!(
            rel.starts_with("usr") || rel.starts_with("etc/porter"),
            "{rel:?}"
        );
        assert!(!path.to_string_lossy().contains(".new."), "{path:?}");
    }
}

#[test]
fn a_caller_table_the_person_changed_is_kept_and_the_shipped_one_is_put_beside_it() {
    let run = Run::new("kept");
    run.install(&[]);
    let table = run.at("/etc/porter/callers.toml");
    fs::write(&table, "# mine\n").expect("edit");
    let out = run.install(&[]);
    assert!(out.contains("kept /etc/porter/callers.toml"), "{out}");
    assert_eq!(fs::read_to_string(&table).expect("read"), "# mine\n");
    assert_eq!(
        fs::read_to_string(run.at("/etc/porter/callers.toml.dist")).expect("dist"),
        fs::read_to_string(repo().join("dist/callers.toml")).expect("shipped")
    );
    // And that is also a state a further run leaves as it is.
    let again = run.install(&[]);
    assert!(!again.contains("installed"), "{again}");
}

#[test]
fn the_script_builds_the_daemons_with_their_default_features_only() {
    let script = fs::read_to_string(repo().join("scripts/install.sh")).expect("script");
    let builds: Vec<_> = script
        .lines()
        .filter(|l| l.contains("cargo build"))
        .collect();
    assert_eq!(builds.len(), 1, "{builds:?}");
    assert!(!builds[0].contains("feature"), "{}", builds[0]);
    assert!(builds[0].contains("--release"));
    for test_only in ["test-keys", "test-proc-root"] {
        assert!(!script.contains(test_only), "{test_only}");
    }
}
