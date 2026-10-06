//! The caller table as files: `/etc/porter/callers.toml` and the user's own, the same files
//! accountd reads (one table for both daemons, PLAN §2.1). The table's type is
//! `porter_dbus::CallerTable`; the reading lives here.

use porter_dbus::CallerTable;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// A table file that exists and cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerFileError {
    /// The file.
    pub path: PathBuf,
    /// What is wrong with it.
    pub message: String,
}

impl std::fmt::Display for CallerFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for CallerFileError {}

fn table_from_file(path: &Path) -> Result<CallerTable, CallerFileError> {
    let failed = |message: String| CallerFileError {
        path: path.to_owned(),
        message,
    };
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| failed(e.to_string())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(CallerTable::default()),
        Err(e) => Err(failed(e.to_string())),
    }
}

/// The system's file with the user's laid over it (the user's rows win). A file that does not
/// parse is an error, not an empty table: a daemon that dropped a bad file would grant less or
/// more than it was told.
pub fn load_callers(system: &Path, user: &Path) -> Result<CallerTable, CallerFileError> {
    Ok(CallerTable::layered(
        table_from_file(system)?,
        table_from_file(user)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::AppName;
    use porter_dbus::CallerRole;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("syncd-callers-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn the_user_file_wins_a_missing_file_is_empty_and_a_broken_one_is_an_error() {
        let dir = scratch("layers");
        let (system, user) = (dir.join("system.toml"), dir.join("user.toml"));
        std::fs::write(
            &system,
            "[[caller]]\napp = \"org.quire.Settings\"\nrole = \"settings\"\n",
        )
        .expect("system");
        std::fs::write(
            &user,
            "[[caller]]\napp = \"org.quire.Settings\"\nrole = \"app\"\n",
        )
        .expect("user");
        let settings = AppName::parse("org.quire.Settings").expect("name");
        let merged = load_callers(&system, &user).expect("load");
        assert_eq!(merged.role_of(&settings), CallerRole::App);
        let alone = load_callers(&system, &dir.join("missing.toml")).expect("alone");
        assert_eq!(alone.role_of(&settings), CallerRole::Settings);
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[[caller]]\napp = \"org.x.A\"\nrole = \"root\"\n").expect("bad");
        let err = load_callers(&bad, &dir.join("nope.toml")).expect_err("unknown role");
        assert_eq!(err.path, bad);
        let _ = std::fs::remove_dir_all(dir);
    }
}
