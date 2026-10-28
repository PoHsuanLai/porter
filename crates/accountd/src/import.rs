//! Moving the AI keys inferd held into accountd's own store, once, when accountd starts.
//!
//! Before AI keys were accountd's, inferd kept a second store: a registry (`registry.json` in
//! `$XDG_STATE_HOME/quire/inferd/keys`), the keys in the Secret Service under `porter.inferd`
//! and an audit file. [`run`] copies it, in this order, and removes nothing until the copy is
//! verified:
//!
//! 1. the old registry is read, and its SHA-256 taken; a record ([`ImportMarker`]) that already
//!    covers that digest means the copy was made;
//! 2. each account's secret is filed in accountd's namespace (`put_if_absent`: what accountd
//!    already holds is never replaced; a different value is counted as a conflict and left);
//! 3. the accounts, grants and switches accountd does not hold are added to its registry, which
//!    is saved once (an id that means a different account refuses the whole import and nothing is
//!    written);
//! 4. the registry is read back from disk and every secret is read back and compared with the old
//!    one in constant time;
//! 5. the old audit lines are appended to accountd's audit file;
//! 6. the record is written;
//! 7. the old secrets are removed (a failure leaves them and the next start tries again).
//!
//! accountd may not write inferd's files, so it never removes them: inferd removes its own old
//! registry and audit file at its start, while the record still covers the registry it finds.
//! Nothing here prints or formats a key; the report is counts.

use porter_core::store::{Persisted, RegistryStore, StoreError};
use porter_core::{AccountId, SecretKey};
use porter_secrets::{Secrets, SecretsError};
use porter_store::{FileStore, ImportMarker, MarkerError};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where inferd kept its store, and where accountd records the copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldStore {
    /// inferd's registry directory (`.../quire/inferd/keys`).
    pub registry_dir: PathBuf,
    /// inferd's audit file (`.../quire/inferd/keys-audit.jsonl`).
    pub audit: PathBuf,
}

/// What an import was asked to do it over.
#[derive(Debug)]
pub struct Import<'a, O, N> {
    /// inferd's old store.
    pub old: &'a OldStore,
    /// The secrets as inferd filed them (the namespace `porter.inferd`).
    pub old_secrets: &'a O,
    /// accountd's secrets.
    pub secrets: &'a N,
    /// accountd's registry.
    pub store: &'a FileStore,
    /// accountd's audit file, which the old lines are appended to.
    pub audit: &'a Path,
    /// The record of the copy.
    pub marker: &'a Path,
}

/// How an import ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// inferd left no registry: nothing to copy.
    NothingToImport,
    /// The record covers the registry that is there: it was copied already.
    AlreadyImported,
    /// The store was copied now.
    Imported(Report),
}

/// What an import did, as counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// Accounts added to accountd's registry.
    pub accounts: usize,
    /// Grants added.
    pub grants: usize,
    /// Switches added.
    pub toggles: usize,
    /// Secrets filed in accountd's namespace.
    pub secrets: usize,
    /// Secrets accountd already held with another value (accountd's was kept).
    pub conflicts: usize,
    /// Audit lines appended.
    pub audit_lines: usize,
}

/// Why an import did not finish. Nothing of inferd's was changed by any of them; the next start
/// tries again.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImportError {
    /// inferd's registry could not be read.
    #[error("inferd's registry could not be read: {0}")]
    Old(String),
    /// accountd's registry could not be read or saved.
    #[error("accountd's registry could not be used: {0}")]
    Registry(StoreError),
    /// A secret store refused.
    #[error("a secret store refused: {0}")]
    Secrets(SecretsError),
    /// An account of inferd has the id of another account of accountd.
    #[error("inferd's account {0} is another account than accountd's of the same id")]
    Conflict(AccountId),
    /// The copy did not read back as it was written.
    #[error("the copy did not verify: {0}")]
    Verify(&'static str),
    /// A file could not be read or written.
    #[error("a file could not be used: {0:?}")]
    Io(std::io::ErrorKind),
    /// The record of the copy cannot be read.
    #[error("{0}")]
    Marker(#[from] MarkerError),
}

impl From<std::io::Error> for ImportError {
    fn from(why: std::io::Error) -> Self {
        Self::Io(why.kind())
    }
}

impl From<SecretsError> for ImportError {
    fn from(why: SecretsError) -> Self {
        Self::Secrets(why)
    }
}

/// The secrets an account of the old registry holds: the one its sign-in files.
fn keys_of(persisted: &Persisted) -> Vec<SecretKey> {
    persisted
        .accounts
        .iter()
        .filter_map(|account| {
            Some(SecretKey {
                account: account.id.clone(),
                purpose: account.auth.facts().secret?,
            })
        })
        .collect()
}

/// The old registry's bytes, or none when inferd left no registry.
fn read_old(store: &FileStore) -> Result<Option<Vec<u8>>, ImportError> {
    match std::fs::read(store.path()) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(why) => Err(why.into()),
    }
}

/// The lines of the old audit file; none when there is no such file.
fn old_audit(path: &Path) -> Result<Vec<String>, ImportError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text.lines().map(str::to_owned).collect()),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(why) => Err(why.into()),
    }
}

/// Appends `lines` to the audit file at `path`, created owner-only with its directory, in one
/// write so that lines of another writer do not interleave.
fn append(path: &Path, lines: &[String]) -> Result<(), ImportError> {
    if lines.is_empty() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut text = lines.join("\n");
    text.push('\n');
    options.open(path)?.write_all(text.as_bytes())?;
    Ok(())
}

/// Whether the audit file at `path` ends with `lines`.
fn ends_with(path: &Path, lines: &[String]) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return lines.is_empty();
    };
    let have: Vec<&str> = text.lines().collect();
    have.len() >= lines.len()
        && have[have.len() - lines.len()..]
            .iter()
            .zip(lines)
            .all(|(a, b)| *a == b)
}

/// Adds to `mine` what `old` holds and `mine` does not, and counts it. An account of the same id
/// that is another account (another provider or another way to sign in) is a conflict that
/// refuses the import.
fn merge(mine: &mut Persisted, old: &Persisted) -> Result<Report, ImportError> {
    let mut report = Report::default();
    for account in &old.accounts {
        match mine.accounts.iter().find(|a| a.id == account.id) {
            Some(held) if held.provider != account.provider || held.auth != account.auth => {
                return Err(ImportError::Conflict(account.id.clone()));
            }
            Some(_) => {}
            None => {
                mine.accounts.push(account.clone());
                report.accounts += 1;
            }
        }
    }
    for grant in &old.grants {
        if !mine.grants.iter().any(|g| g.id == grant.id) {
            mine.grants.push(grant.clone());
            report.grants += 1;
        }
    }
    for toggle in &old.toggles {
        if !mine
            .toggles
            .iter()
            .any(|t| t.account == toggle.account && t.kind == toggle.kind)
        {
            mine.toggles.push(toggle.clone());
            report.toggles += 1;
        }
    }
    Ok(report)
}

/// Whether everything `old` holds is in `mine`, as it was.
fn holds_all(mine: &Persisted, old: &Persisted) -> bool {
    old.accounts
        .iter()
        .all(|account| mine.accounts.iter().any(|a| a.id == account.id))
        && old
            .grants
            .iter()
            .all(|grant| mine.grants.iter().any(|g| g == grant))
        && old.toggles.iter().all(|toggle| {
            mine.toggles
                .iter()
                .any(|t| t.account == toggle.account && t.kind == toggle.kind)
        })
}

impl<O: Secrets, N: Secrets> Import<'_, O, N> {
    /// Copies inferd's store into accountd's, as the module says.
    ///
    /// # Errors
    /// See [`ImportError`].
    pub async fn run(&self) -> Result<Outcome, ImportError> {
        let old_store = FileStore::new(&self.old.registry_dir);
        let Some(bytes) = read_old(&old_store)? else {
            return Ok(Outcome::NothingToImport);
        };
        if let Some(done) = ImportMarker::read(self.marker)?
            && done.covers(&bytes)
        {
            if !done.secrets_removed {
                let old = parse(&bytes)?;
                self.remove_old_secrets(&old).await?;
                let mut done = done;
                done.secrets_removed = true;
                done.write(self.marker)?;
            }
            return Ok(Outcome::AlreadyImported);
        }
        let old = parse(&bytes)?;
        let keys = keys_of(&old);
        // What accountd would hold, worked out before anything is written: an id that means
        // another account refuses the import with nothing changed.
        let mut mine = self.store.load().await.map_err(ImportError::Registry)?;
        let counted = merge(&mut mine, &old)?;
        let mut report = Report {
            accounts: counted.accounts,
            grants: counted.grants,
            toggles: counted.toggles,
            ..Report::default()
        };
        let mut kept: Vec<SecretKey> = Vec::new();
        // The secrets first: a registry that names an account whose key was not copied would be
        // an account that has to be signed in again.
        for key in &keys {
            let value = match self.old_secrets.get(key).await {
                Ok(value) => value,
                // An account whose key is gone stays an account; it needs signing in again.
                Err(SecretsError::Missing | SecretsError::Unreadable) => continue,
                Err(why) => return Err(why.into()),
            };
            if self.secrets.put_if_absent(key, &value).await? {
                report.secrets += 1;
            } else if self.secrets.get(key).await? != value {
                report.conflicts += 1;
                kept.push(key.clone());
            }
        }
        if counted != Report::default() {
            self.store
                .save(&mine)
                .await
                .map_err(ImportError::Registry)?;
        }
        self.verify(&old, &keys, &kept).await?;
        let lines = old_audit(&self.old.audit)?;
        append(self.audit, &lines)?;
        if !ends_with(self.audit, &lines) {
            return Err(ImportError::Verify("the audit lines"));
        }
        report.audit_lines = lines.len();
        let mut done = ImportMarker::of(&bytes);
        done.accounts = old.accounts.len();
        done.grants = old.grants.len();
        done.toggles = old.toggles.len();
        done.secrets = report.secrets;
        done.audit_lines = report.audit_lines;
        done.write(self.marker)?;
        self.remove_old_secrets(&old).await?;
        done.secrets_removed = true;
        done.write(self.marker)?;
        Ok(Outcome::Imported(report))
    }

    /// Reads accountd's registry and secrets back and checks they hold what inferd's did. A
    /// secret accountd held already under another value (`kept`) is accountd's own.
    async fn verify(
        &self,
        old: &Persisted,
        keys: &[SecretKey],
        kept: &[SecretKey],
    ) -> Result<(), ImportError> {
        let mine = self.store.load().await.map_err(ImportError::Registry)?;
        if !holds_all(&mine, old) {
            return Err(ImportError::Verify("the registry"));
        }
        for key in keys.iter().filter(|key| !kept.contains(key)) {
            let before = match self.old_secrets.get(key).await {
                Ok(value) => value,
                Err(SecretsError::Missing | SecretsError::Unreadable) => continue,
                Err(why) => return Err(why.into()),
            };
            // `Credential` is compared in constant time, through its `SecretText`s.
            match self.secrets.get(key).await {
                Ok(after) if after == before => {}
                _ => return Err(ImportError::Verify("a secret")),
            }
        }
        Ok(())
    }

    /// Removes inferd's items: the copy is verified and recorded.
    async fn remove_old_secrets(&self, old: &Persisted) -> Result<(), ImportError> {
        for key in keys_of(old) {
            self.old_secrets.delete(&key).await?;
        }
        Ok(())
    }
}

fn parse(bytes: &[u8]) -> Result<Persisted, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|why| ImportError::Old(why.to_string()))?;
    Persisted::from_json(text).map_err(|why| ImportError::Old(why.to_string()))
}

#[cfg(test)]
mod tests;
