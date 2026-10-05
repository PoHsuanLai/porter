//! The caller table as files: `/etc/porter/callers.toml` and the user's own, read from the
//! paths the daemon's main passes in.

use super::CallerTable;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// A table file that exists and cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableFileError {
    /// The file.
    pub path: PathBuf,
    /// What is wrong with it.
    pub message: String,
}

impl std::fmt::Display for TableFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for TableFileError {}

impl CallerTable {
    /// The table in TOML text: `[[caller]]` rows of `exe`, `app` and `role`.
    pub fn from_toml_text(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// The table in the file `path`; a file that is not there is an empty table.
    pub fn from_file(path: &Path) -> Result<Self, TableFileError> {
        let failed = |message: String| TableFileError {
            path: path.to_owned(),
            message,
        };
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml_text(&text).map_err(failed),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(failed(e.to_string())),
        }
    }

    /// The table of the system's file with the user's laid over it. A file that does not parse
    /// is an error, not an empty table: a daemon that dropped a bad user file would grant less
    /// or more than it was told.
    pub fn load(system: &Path, user: &Path) -> Result<Self, TableFileError> {
        Ok(Self::layered(
            Self::from_file(system)?,
            Self::from_file(user)?,
        ))
    }
}
