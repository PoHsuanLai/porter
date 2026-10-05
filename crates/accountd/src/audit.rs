//! accountd's audit file (design/31 §4.5): `audit.jsonl`, one [`AuditEntry`] per line,
//! append-only, in the shape of inferd's (`crates/inferd/src/audit.rs`). An entry holds ids,
//! kinds and endpoints, never a value, so the file cannot leak a secret.

use porter_core::audit::AuditEntry;
use porter_service::AuditSink;
use std::io::Write;
use std::path::PathBuf;

/// Appends entries to a JSON-lines file that is created, mode 0600, with its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAudit {
    path: PathBuf,
}

impl FileAudit {
    /// Appends to `path` (`$XDG_STATE_HOME/quire/accountd/audit.jsonl`, resolved by the caller).
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn append(&self, entry: &AuditEntry) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        line.push('\n');
        // One write call per line, so lines never interleave under O_APPEND.
        options.open(&self.path)?.write_all(line.as_bytes())
    }
}

impl AuditSink for FileAudit {
    fn record(&self, entry: AuditEntry) {
        if let Err(why) = self.append(&entry) {
            // The daemon's one log path is standard error, prefixed with its name.
            eprintln!("accountd: audit: {}: {why}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests;
