//! The identity of the rig's client, as the daemons read it.
//!
//! accountd, syncd and inferd name the process behind a bus connection from `<proc root>/<pid>`
//! (the bus gives the pid): its `cgroup`, and for a Flatpak app its sandbox's metadata,
//! `root/.flatpak-info` (a Flatpak scope's name alone proves nothing). A service unit's role goes
//! only to the unit's main process, which a fixture tree states in `<proc root>/units/<unit>`
//! (the decimal pid) where the real daemons ask the systemd manager. A build with the
//! `test-proc-root` feature reads `<proc root>` from `ACCOUNTD_PROC_ROOT`, `SYNCD_PROC_ROOT` or
//! `INFERD_PROC_ROOT` instead of `/proc`, and a release build ignores the variable. A scenario
//! therefore:
//!
//! 1. makes a directory, `proc`, and starts each daemon (a `test-proc-root` build) with
//!    `ACCOUNTD_PROC_ROOT=<dir>/proc` (and the other two);
//! 2. runs `porter-rig-client --app-id org.example.App --proc-root <dir>/proc <command>`, which
//!    writes `<dir>/proc/<its pid>/cgroup` and `root/.flatpak-info` naming it a Flatpak app
//!    `org.example.App` before it connects and removes them when it ends; or, for an app that is
//!    some other process, runs
//!    `porter-rig-client --app-id org.example.App --proc-root <dir>/proc write-identity --pid N`.
//!
//! The role of the app (App, Settings, SheetHost, PorterDaemon, ...) is the daemons' own table
//! (`callers.toml`), not something the fixture says.

use std::io;
use std::path::{Path, PathBuf};

/// A unit-name component with the bytes systemd escapes written as `\xHH`.
fn escape(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'-' => "\\x2d".to_owned(),
            b if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' => char::from(b).to_string(),
            b => format!("\\x{b:02x}"),
        })
        .collect()
}

/// The text of `<pid>/cgroup` for Flatpak app `app`.
pub fn cgroup_text(app: &str, pid: u32) -> String {
    format!(
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-{}-{pid}.scope\n",
        escape(app)
    )
}

/// The text of `<pid>/root/.flatpak-info` for Flatpak app `app`.
pub fn flatpak_info_text(app: &str, pid: u32) -> String {
    format!("[Application]\nname={app}\n\n[Instance]\ninstance-id={pid}\n")
}

/// A written identity. Dropping it removes it.
#[derive(Debug)]
pub struct Identity {
    dir: PathBuf,
}

impl Identity {
    /// Names process `pid` Flatpak app `app` under `proc_root`: its scope and its sandbox's
    /// metadata.
    pub fn write(proc_root: &Path, app: &str, pid: u32) -> io::Result<Self> {
        let dir = proc_root.join(pid.to_string());
        std::fs::create_dir_all(dir.join("root"))?;
        std::fs::write(dir.join("cgroup"), cgroup_text(app, pid))?;
        std::fs::write(
            dir.join("root").join(".flatpak-info"),
            flatpak_info_text(app, pid),
        )?;
        Ok(Self { dir })
    }

    /// Keeps the identity after the guard is gone (an app that outlives the command).
    pub fn keep(self) {
        std::mem::forget(self);
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_name_escapes_the_dash_and_keeps_dots() {
        const CASES: &[(&str, &str)] = &[
            ("org.quire.Photos", "org.quire.Photos"),
            ("org.example.My-App", "org.example.My\\x2dApp"),
            ("a_b", "a_b"),
        ];
        for (app, escaped) in CASES {
            assert_eq!(escape(app), *escaped, "{app}");
        }
    }

    #[test]
    fn the_cgroup_names_a_flatpak_scope_the_daemons_parse_back() {
        let text = cgroup_text("org.example.My-App", 4242);
        assert!(
            text.trim_end()
                .ends_with("app-flatpak-org.example.My\\x2dApp-4242.scope")
        );
        let root = std::env::temp_dir().join(format!("rig-fixture-{}", std::process::id()));
        let identity = Identity::write(&root, "org.example.My-App", 4242).expect("write");
        let table = porter_dbus::CallerTable::default();
        let caller =
            porter_dbus::ProcCallers::caller_of_pid(&root, 4242, &table).expect("a caller");
        assert_eq!(caller.app.name.as_str(), "org.example.My-App");
        drop(identity);
        assert!(porter_dbus::ProcCallers::caller_of_pid(&root, 4242, &table).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
