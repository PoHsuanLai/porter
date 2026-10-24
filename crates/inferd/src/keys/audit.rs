//! inferd's key audit file: `audit.jsonl` beside its other state, one entry per line, append-only,
//! mode 0600. An entry holds ids, kinds and endpoints, never a value, so the file cannot leak a
//! secret.

use porter_service::AuditSink;
use porter_service::audit::AuditEntry;
use std::io::Write;
use std::path::PathBuf;

/// Appends entries to a JSON-lines file that is created with its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyAudit {
    path: PathBuf,
}

impl KeyAudit {
    /// Appends to `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn append(&self, entry: &AuditEntry) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
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

impl AuditSink for KeyAudit {
    fn record(&self, entry: AuditEntry) {
        if let Err(why) = self.append(&entry) {
            eprintln!("inferd: audit: {}: {why}", self.path.display());
        }
    }
}
