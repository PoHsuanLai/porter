//! The configuration file as the settings writer and the daemon share it: read whole, edited at a
//! settings path, written atomically; and the reload that puts a changed file in force.

use super::resolve::resolve;
use crate::config::InferdConfig;
use crate::engines::Engines;
use crate::supervise::Programs;
use porter_fs::atomic::AtomicWrite;
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
    before: Option<PathBuf>,
}

impl ConfigFile {
    /// The file at `path`; it need not exist yet.
    pub fn new(path: PathBuf) -> Self {
        Self { path, before: None }
    }

    /// The file that was at `before` until it moved to `path`: read while `path` is missing, never
    /// written. The first change a person makes writes the whole of it to `path`.
    #[must_use]
    pub fn moved_from(mut self, before: PathBuf) -> Self {
        self.before = Some(before);
        self
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file to read: `path`, or, while only the file at its old place is there, that one.
    pub fn source(&self) -> &Path {
        match &self.before {
            Some(before) if !self.path.exists() && before.exists() => before,
            _ => &self.path,
        }
    }

    fn text(&self) -> Result<String, FileError> {
        let source = self.source();
        match std::fs::read_to_string(source) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(FileError::Io(format!("{}: {e}", source.display()))),
        }
    }

    /// The configuration in the file; a missing file is the default.
    pub fn read(&self) -> Result<InferdConfig, FileError> {
        InferdConfig::from_toml(&self.text()?)
            .map_err(|e| FileError::Toml(format!("{}: {e}", self.source().display())))
    }

    /// Sets the row at the dotted settings `path` to `value` and writes the file (to a sibling,
    /// then renamed over it). The file is rewritten from its parsed form, so its comments are not
    /// kept; a file that does not parse is left alone and is the error.
    pub fn set(&self, path: &str, value: toml::Value) -> Result<(), FileError> {
        self.edit(|table| put(table, path, value))
    }

    /// Sets `key` in the table at the dotted `table_path` to `value`, or removes it when `value`
    /// is `None`. `key` is one key even where it holds dots (a helper's pool is
    /// `"text.helper.<id>"` in `[ai.auto.pool]`). Written as `set` writes.
    pub fn set_key(
        &self,
        table_path: &str,
        key: &str,
        value: Option<toml::Value>,
    ) -> Result<(), FileError> {
        self.edit(|table| {
            let inner = table_at(table, table_path);
            match value {
                Some(value) => {
                    inner.insert(key.to_owned(), value);
                }
                None => {
                    inner.remove(key);
                }
            }
        })
    }

    /// Reads the file whole, lets `change` edit it, and writes it back as `set` describes.
    fn edit(&self, change: impl FnOnce(&mut toml::Table)) -> Result<(), FileError> {
        let mut table: toml::Table = self.text()?.parse().map_err(|e: toml::de::Error| {
            FileError::Toml(format!("{}: {e}", self.path.display()))
        })?;
        change(&mut table);
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
        let meta = std::fs::metadata(self.source()).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }
}

/// The table at the dotted `path` in `table`, made (over anything else there) on the way.
fn table_at<'a>(table: &'a mut toml::Table, path: &str) -> &'a mut toml::Table {
    path.split('.').fold(table, |table, name| {
        let entry = table
            .entry(name.to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        entry.as_table_mut().expect("a table, made just above")
    })
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
    /// Held by whoever reads the file, changes it and reloads, so two writers (the settings
    /// module and the helpers' pools) never overwrite each other.
    writing: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// The programs the engines are started with, when the daemon lets the file change them.
    programs: Option<Programs>,
}

impl Reload {
    /// Reloads `engines` from `file`.
    pub fn new(file: ConfigFile, engines: Engines) -> Self {
        Self {
            file,
            engines,
            writing: Default::default(),
            programs: None,
        }
    }

    /// The same, also telling `programs` which program of an engine the file names, so the next
    /// start of an engine uses it.
    #[must_use]
    pub fn with_programs(self, programs: Programs) -> Self {
        Self {
            programs: Some(programs),
            ..self
        }
    }

    /// The file.
    pub fn file(&self) -> &ConfigFile {
        &self.file
    }

    /// The lock of the one writer at a time, shared by every clone.
    pub fn writing(&self) -> &tokio::sync::Mutex<()> {
        &self.writing
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
        if let Some(programs) = &self.programs {
            programs.follow(&config.engines);
        }
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
