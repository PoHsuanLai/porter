//! The caller table as files: `/etc/porter/callers.toml` and the user's own, at the paths a
//! daemon's main passes in. One reader for every daemon (accountd and syncd read the same two
//! files, PLAN §2.1); the table's type is [`CallerTable`]. Behind the `callers-file` feature,
//! which brings the TOML reader in: an app that only calls the daemons does not need it.

use crate::CallerTable;
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

/// The table in TOML text: `[[caller]]` rows of `app`, optional `unit` and `role`.
pub fn table_from_toml(text: &str) -> Result<CallerTable, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// The table in the file `path`; a file that is not there is an empty table.
pub fn table_from_file(path: &Path) -> Result<CallerTable, CallerFileError> {
    let failed = |message: String| CallerFileError {
        path: path.to_owned(),
        message,
    };
    match std::fs::read_to_string(path) {
        Ok(text) => table_from_toml(&text).map_err(failed),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(CallerTable::default()),
        Err(e) => Err(failed(e.to_string())),
    }
}

/// The system's file with the user's laid over it (the user's rows win). A file that does not
/// parse is an error, not an empty table: a daemon that dropped a bad user file would grant
/// less or more than it was told.
pub fn load_callers(system: &Path, user: &Path) -> Result<CallerTable, CallerFileError> {
    Ok(CallerTable::layered(
        table_from_file(system)?,
        table_from_file(user)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CallerRole;
    use porter_core::AppName;

    const SYSTEM: &str = "[[caller]]\napp = \"org.quire.Settings\"\nrole = \"settings\"\n\n\
        [[caller]]\napp = \"org.quire.Inference\"\nunit = \"inferd.service\"\nrole = \"porter_daemon\"\n\n\
        [[caller]]\napp = \"org.example.Sheets\"\nrole = \"sheet_host\"\n";
    const USER: &str = "[[caller]]\napp = \"org.quire.Settings\"\nrole = \"app\"\n\n\
        [[caller]]\napp = \"org.example.Mine\"\nunit = \"inferd.service\"\nrole = \"app\"\n";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("callers-file-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn name(text: &str) -> AppName {
        AppName::parse(text).expect("name")
    }

    #[test]
    fn the_user_file_wins_over_the_system_file() {
        let dir = scratch("layer");
        let (system, user) = (dir.join("system.toml"), dir.join("user.toml"));
        std::fs::write(&system, SYSTEM).expect("system");
        std::fs::write(&user, USER).expect("user");
        let merged = load_callers(&system, &user).expect("load");
        let unit = merged.resolve_unit("inferd.service").expect("unit");
        assert_eq!(unit.app.name.as_str(), "org.example.Mine");
        assert_eq!(unit.role, CallerRole::App);
        assert_eq!(merged.role_of(&name("org.quire.Settings")), CallerRole::App);
        assert_eq!(
            merged.role_of(&name("org.example.Sheets")),
            CallerRole::SheetHost
        );
        let alone = load_callers(&system, &dir.join("missing.toml")).expect("alone");
        assert_eq!(
            alone.role_of(&name("org.quire.Settings")),
            CallerRole::Settings
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_is_empty_and_a_broken_one_is_an_error() {
        let dir = scratch("broken");
        assert_eq!(
            table_from_file(&dir.join("nope.toml")),
            Ok(CallerTable::default())
        );
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[[caller]]\napp = \"org.x.A\"\nrole = \"root\"\n").expect("bad");
        let err = table_from_file(&bad).expect_err("unknown role");
        assert_eq!(err.path, bad);
        assert!(load_callers(&bad, &dir.join("nope.toml")).is_err());
        assert!(table_from_toml("[[caller]]\napp = \"not a name\"\nrole = \"app\"\n").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_row_may_carry_the_name_a_person_reads() {
        let table = table_from_toml(
            "[[caller]]\napp = \"org.quire.Sync\"\nunit = \"syncd.service\"\nrole = \"porter_daemon\"\nname = \"Sync\"\n\n\
             [[caller]]\napp = \"org.quire.Settings\"\nrole = \"settings\"\n",
        )
        .expect("table");
        assert_eq!(
            table.title_of(&name("org.quire.Sync")),
            Some(&crate::AppTitle("Sync".to_owned()))
        );
        assert_eq!(table.title_of(&name("org.quire.Settings")), None);
        assert!(
            table_from_toml("[[caller]]\napp = \"org.x.A\"\nrole = \"app\"\nname = 3\n").is_err()
        );
    }
}
