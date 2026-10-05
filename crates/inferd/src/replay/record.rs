//! The replay engine's request log: with `record = "<file>"` in the engine's table, every
//! request body the engine receives is appended as one JSON line. THIS WRITES PROMPTS TO DISK,
//! so the file is created 0600, the path comes from the table and nowhere else, and the key is
//! accepted only on a replay engine. For acceptance runs that must show what the model was sent.

use serde_json::Value;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

/// Appends request bodies to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorder {
    path: PathBuf,
}

impl Recorder {
    /// Records to `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Appends `body` as one line. A failure is reported on standard error; a request never
    /// fails on it.
    pub fn append(&self, body: &Value) {
        if let Err(why) = self.write(body) {
            eprintln!("inferd: record: {}: {why}", self.path.display());
        }
    }

    fn write(&self, body: &Value) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        // A file that was there already may be wider than 0600; the prompts in it are not
        // anyone else's.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        let mut line = serde_json::to_string(body).map_err(std::io::Error::other)?;
        line.push('\n');
        file.write_all(line.as_bytes())
    }
}

#[cfg(test)]
mod tests;
