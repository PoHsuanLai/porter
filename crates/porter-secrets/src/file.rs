//! A file-backed store for private-bus and scratch runs: TEST ONLY, behind the non-default
//! `test-keys` feature, never compiled into a default or release build.
//!
//! One JSON file holds every credential as plain text, so it protects nothing but the file's
//! mode: it is created `0600` and refused, on open and on every later use, when any group or
//! other bit is set. Items are addressed as the keyring backends address them (`service` the
//! store's namespace, `porter` unless the host names another; the account; the purpose in its
//! serde form). Every use takes an advisory lock on a
//! sibling `<file>.lock`, reads the file and, for a change, writes it through `porter_core`'s
//! atomic writer (a staging file, synced, renamed over the file), so a crash leaves the old file
//! whole and writers in this or any other process
//! (a daemon and `accountd add`) are serialised. The I/O is synchronous: the files are tiny and
//! nothing awaits while the lock is held.

use crate::attributes::attributes_in;
use crate::error::SecretsError;
use crate::namespace::Namespace;
use crate::secrets::Secrets;
use porter_core::AccountId;
use porter_core::{Credential, SecretKey};
use porter_fs::atomic::AtomicWrite;
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The file format's version, the `version` member of the document.
const VERSION: u64 = 1;

/// Why the key file could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FileSecretsError {
    /// The path is not absolute.
    #[error("the key file path is not absolute")]
    NotAbsolute,
    /// The file is not a regular file.
    #[error("the key file is not a regular file")]
    NotAFile,
    /// The file is readable or writable by group or other; `mode` is its permission bits.
    #[error("the key file is accessible to group or other (mode {mode:o}); it must be 0600")]
    Mode {
        /// The file's permission bits.
        mode: u32,
    },
    /// The file is not a key file this build reads.
    #[error("the key file does not parse as a key file")]
    Corrupt,
    /// The file system refused.
    #[error("the key file could not be read or written: {0:?}")]
    Io(ErrorKind),
}

impl From<std::io::Error> for FileSecretsError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}

impl From<FileSecretsError> for SecretsError {
    fn from(error: FileSecretsError) -> Self {
        match error {
            FileSecretsError::Corrupt => SecretsError::Unreadable,
            FileSecretsError::NotAbsolute
            | FileSecretsError::NotAFile
            | FileSecretsError::Mode { .. }
            | FileSecretsError::Io(_) => SecretsError::Unavailable,
        }
    }
}

/// One filed item.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    service: String,
    account: String,
    purpose: String,
    credential: Value,
}

impl Item {
    /// The item `key` is filed as in `namespace`.
    fn new(namespace: &Namespace, key: &SecretKey, credential: Value) -> Self {
        let at = attributes_in(namespace, key);
        Self {
            service: at.service,
            account: at.account,
            purpose: at.purpose,
            credential,
        }
    }

    fn is(&self, namespace: &Namespace, key: &SecretKey) -> bool {
        let at = attributes_in(namespace, key);
        self.service == at.service && self.account == at.account && self.purpose == at.purpose
    }

    fn to_json(&self) -> Value {
        json!({
            "service": self.service,
            "account": self.account,
            "purpose": self.purpose,
            "credential": self.credential,
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        let text = |name: &str| value.get(name)?.as_str().map(str::to_owned);
        Some(Self {
            service: text("service")?,
            account: text("account")?,
            purpose: text("purpose")?,
            credential: value.get("credential")?.clone(),
        })
    }
}

fn parse(text: &str) -> Result<Vec<Item>, FileSecretsError> {
    let root: Value = serde_json::from_str(text).map_err(|_| FileSecretsError::Corrupt)?;
    if root.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err(FileSecretsError::Corrupt);
    }
    root.get("items")
        .and_then(Value::as_array)
        .ok_or(FileSecretsError::Corrupt)?
        .iter()
        .map(|item| Item::from_json(item).ok_or(FileSecretsError::Corrupt))
        .collect()
}

fn render(items: &[Item]) -> Vec<u8> {
    let document = json!({
        "version": VERSION,
        "items": items.iter().map(Item::to_json).collect::<Vec<_>>(),
    });
    // A `Value` always serializes.
    serde_json::to_vec_pretty(&document).unwrap_or_default()
}

/// Credentials in one `0600` JSON file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSecrets {
    path: PathBuf,
    namespace: Namespace,
}

impl FileSecrets {
    /// The store in the file at `path` (absolute), filing under [`Namespace::PORTER`]: created
    /// empty with mode `0600` (and its directories) if missing, refused if it is not a regular
    /// file, has a group or other permission bit set, or does not parse.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, FileSecretsError> {
        Self::open_in(path, Namespace::PORTER)
    }

    /// The store in the file at `path`, filing under `namespace`: [`FileSecrets::open`] for a
    /// host with a namespace of its own. Items of other namespaces in the same file are kept
    /// and never read.
    pub fn open_in(
        path: impl Into<PathBuf>,
        namespace: Namespace,
    ) -> Result<Self, FileSecretsError> {
        let store = Self {
            path: path.into(),
            namespace,
        };
        if !store.path.is_absolute() {
            return Err(FileSecretsError::NotAbsolute);
        }
        store.update(|_| ((), false))?;
        Ok(store)
    }

    /// The namespace items are filed under.
    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// The file this store keeps its items in.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    /// Runs `change` over the items under the exclusive lock, writing them back if it answers
    /// `true` as the second half: that it changed them.
    fn update<R>(
        &self,
        change: impl FnOnce(&mut Vec<Item>) -> (R, bool),
    ) -> Result<R, FileSecretsError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(self.sibling(".lock"))?;
        lock.lock()?;
        // The lock is released when `lock` drops, on every path out.
        let mut items = self.read()?;
        let (out, changed) = change(&mut items);
        if changed {
            self.write(&render(&items))?;
        }
        Ok(out)
    }

    /// The items, creating an empty file if there is none. Checks the mode of the open file.
    fn read(&self) -> Result<Vec<Item>, FileSecretsError> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.write(&render(&[]))?;
                File::open(&self.path)?
            }
            Err(error) => return Err(error.into()),
        };
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(FileSecretsError::NotAFile);
        }
        if meta.mode() & 0o077 != 0 {
            return Err(FileSecretsError::Mode {
                mode: meta.mode() & 0o777,
            });
        }
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|_| FileSecretsError::Corrupt)?;
        parse(&text)
    }

    /// Replaces the store's file with `bytes`, mode `0600`, through the shared atomic writer: a
    /// staging file of its own name (a stale one from a crash is left alone), synced, renamed.
    fn write(&self, bytes: &[u8]) -> Result<(), FileSecretsError> {
        AtomicWrite::PRIVATE
            .write(&self.path, bytes)
            .map_err(Into::into)
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).mode(0o600);
    options
}

impl Secrets for FileSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        let credential = serde_json::to_value(value).map_err(|_| SecretsError::Unreadable)?;
        let namespace = &self.namespace;
        Ok(self.update(|items| {
            items.retain(|item| !item.is(namespace, key));
            items.push(Item::new(namespace, key, credential));
            ((), true)
        })?)
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<bool, SecretsError> {
        let credential = serde_json::to_value(value).map_err(|_| SecretsError::Unreadable)?;
        let namespace = &self.namespace;
        Ok(self.update(|items| {
            if items.iter().any(|item| item.is(namespace, key)) {
                return (false, false);
            }
            items.push(Item::new(namespace, key, credential));
            (true, true)
        })?)
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        let namespace = &self.namespace;
        let found = self.update(|items| {
            (
                items.iter().find(|item| item.is(namespace, key)).cloned(),
                false,
            )
        })?;
        let item = found.ok_or(SecretsError::Missing)?;
        serde_json::from_value(item.credential).map_err(|_| SecretsError::Unreadable)
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        let namespace = &self.namespace;
        Ok(self.update(|items| {
            let before = items.len();
            items.retain(|item| !item.is(namespace, key));
            ((), items.len() != before)
        })?)
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        let account = account.to_string();
        let service = self.namespace.as_str();
        Ok(self.update(|items| {
            let before = items.len();
            items.retain(|item| !(item.service == service && item.account == account));
            ((), items.len() != before)
        })?)
    }
}
