//! The configuration file as the settings writer and the daemon share it: read whole, edited at a
//! settings path, written atomically; and the reload that puts a changed file in force.

use super::resolve::resolve;
use crate::config::InferdConfig;
use crate::engines::Engines;
use porter_core::atomic::AtomicWrite;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Why the file could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    /// The file or its directory could not be read or written.
    #[error("{0}")]
    Io(String),
    /// The file is not valid configuration.
    #[error("{0}")]
    Toml(String),
}

/// `inferd.toml` at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    /// The file at `path`; it need not exist yet.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn text(&self) -> Result<String, FileError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(FileError::Io(format!("{}: {e}", self.path.display()))),
        }
    }

    /// The configuration in the file; a missing file is the default.
    pub fn read(&self) -> Result<InferdConfig, FileError> {
        InferdConfig::from_toml(&self.text()?)
            .map_err(|e| FileError::Toml(format!("{}: {e}", self.path.display())))
    }

    /// Sets the row at the dotted settings `path` to `value` and writes the file (to a sibling,
    /// then renamed over it). The file is rewritten from its parsed form, so its comments are not
    /// kept; a file that does not parse is left alone and is the error.
    pub fn set(&self, path: &str, value: toml::Value) -> Result<(), FileError> {
        let mut table: toml::Table = self.text()?.parse().map_err(|e: toml::de::Error| {
            FileError::Toml(format!("{}: {e}", self.path.display()))
        })?;
        put(&mut table, path, value);
        let text = toml::to_string(&table).map_err(|e| FileError::Toml(e.to_string()))?;
        InferdConfig::from_toml(&text).map_err(|e| FileError::Toml(e.to_string()))?;
        let io = |e: std::io::Error| FileError::Io(format!("{}: {e}", self.path.display()));
        AtomicWrite::SHARED
            .write(&self.path, text.as_bytes())
            .map_err(io)
    }

    /// The file's modification time and length, `None` while it is missing: what the watch
    /// compares.
    fn stamp(&self) -> Option<(SystemTime, u64)> {
        let meta = std::fs::metadata(&self.path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }
}

/// Sets `table` at the dotted `path`, making the tables on the way.
pub(super) fn put(table: &mut toml::Table, path: &str, value: toml::Value) {
    match path.split_once('.') {
        None => {
            table.insert(path.to_owned(), value);
        }
        Some((head, rest)) => {
            let entry = table
                .entry(head.to_owned())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if !entry.is_table() {
                *entry = toml::Value::Table(toml::Table::new());
            }
            if let toml::Value::Table(inner) = entry {
                put(inner, rest, value);
            }
        }
    }
}

/// Why the settings were not reloaded: the old ones stay in force.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ReloadFailed(pub String);

/// What a reload put in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reloaded {
    /// The rows whose values were refused.
    pub rejected: Vec<String>,
}

/// The way a changed file reaches the engines.
#[derive(Debug, Clone)]
pub struct Reload {
    file: ConfigFile,
    engines: Engines,
}

impl Reload {
    /// Reloads `engines` from `file`.
    pub fn new(file: ConfigFile, engines: Engines) -> Self {
        Self { file, engines }
    }

    /// The file.
    pub fn file(&self) -> &ConfigFile {
        &self.file
    }

    /// The engines it reloads.
    pub fn engines(&self) -> &Engines {
        &self.engines
    }

    /// Reads the file and puts what it says in force. A file that does not read changes nothing.
    pub fn now(&self) -> Result<Reloaded, ReloadFailed> {
        let config = self.file.read().map_err(|e| ReloadFailed(e.to_string()))?;
        let resolved = resolve(&config);
        self.engines.apply(resolved.settings);
        Ok(Reloaded {
            rejected: resolved.rejected,
        })
    }

    /// Watches the file: every `every`, when its modification time or length is not what it was,
    /// reloads and says on standard error what it did.
    pub fn watch(self, every: Duration) -> tokio::task::JoinHandle<()> {
        let mut seen = self.file.stamp();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let now = self.file.stamp();
                if now == seen {
                    continue;
                }
                seen = now;
                match self.now() {
                    Ok(done) => {
                        eprintln!("inferd: {}: reloaded", self.file.path().display());
                        for path in done.rejected {
                            eprintln!("inferd: {path}: not accepted; using its default");
                        }
                    }
                    Err(why) => eprintln!("inferd: not reloaded, keeping the settings: {why}"),
                }
            }
        })
    }
}
