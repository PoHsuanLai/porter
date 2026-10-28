//! What accountd keeps of an older registry. Before the split of the accounts service, accountd's
//! `registry.json` also held the AI keys, the local runtimes, the agent logins, the grants and
//! the switches; those belong to inferd and syncd now, which start with files of their own, and
//! accountd keeps the identity accounts (OAuth, password, app password) and the Tailscale one.
//!
//! [`load_identities`] reads the file and drops the rest. Before it does, it keeps the file as
//! it was in `registry.pre-split.json` beside it, so nothing the person had is lost: the copy is
//! made once, never over one that is there, and before accountd's first write could replace the
//! file. A file that has nothing to drop is not copied.

use porter_core::store::{Persisted, prune_to_identities};
use porter_core::store::{RegistryStore, StoreError};
use porter_store::FileStore;
use std::path::PathBuf;

/// The copy of the registry from before the split, in the registry's directory.
pub const PRE_SPLIT_FILE: &str = "registry.pre-split.json";

/// Where the copy of `store`'s registry is kept.
pub fn pre_split_path(store: &FileStore) -> PathBuf {
    store.dir().join(PRE_SPLIT_FILE)
}

/// The registry in `store`, without what accountd no longer holds. When something is dropped the
/// file is copied to [`pre_split_path`] first (unless a copy is there already); if it cannot be
/// copied nothing is dropped and the load fails, so the person never loses an account over it.
///
/// # Errors
/// The registry file cannot be read, or the copy cannot be made.
pub async fn load_identities(store: &FileStore) -> Result<Persisted, StoreError> {
    let stored = store.load().await?;
    let mut kept = stored.clone();
    prune_to_identities(&mut kept);
    if kept == stored {
        return Ok(kept);
    }
    let (from, to) = (store.path(), pre_split_path(store));
    tokio::task::spawn_blocking(move || keep_copy(&from, &to))
        .await
        .map_err(|_| StoreError::Unavailable)?
        .map_err(|_| StoreError::Unavailable)?;
    Ok(kept)
}

/// Copies `from` to `to` unless `to` is there already.
fn keep_copy(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    match to.try_exists()? {
        true => Ok(()),
        false => std::fs::copy(from, to).map(|_| ()),
    }
}

#[cfg(test)]
mod tests;
