//! Removing what inferd kept of the AI keys before accountd did.
//!
//! inferd used to keep a registry of AI accounts and permits in `$XDG_STATE_HOME/quire/inferd/keys`
//! and an audit file beside it; accountd keeps them now, and copies what it finds there when it
//! starts. accountd may not write inferd's state, so inferd removes its own old files, but only
//! when accountd's record of the copy ([`porter_store::ImportMarker`]) covers the registry that
//! is there now: a registry that changed since the copy, a record that is missing, and one that
//! cannot be read all leave the files where they are, and one line says so.
//!
//! Nothing here follows a link: a `keys` that is a link, or a file in it that is not a plain file,
//! is not removed. This is migration code: delete it two releases after accountd keeps the keys.

use crate::config::Dirs;
use porter_store::ImportMarker;
use std::path::Path;

/// What [`retire`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Retired {
    /// There was no old registry.
    NothingThere,
    /// The old registry and audit file were removed: accountd has copied them.
    Removed,
    /// They were left: accountd has not recorded a copy of the registry that is there now.
    Kept,
}

/// Removes the old files when accountd's record covers them. Said on standard error, never a key.
pub fn retire(dirs: &Dirs) -> Retired {
    let retired = retire_at(&dirs.old_keys, &dirs.old_keys_audit, &dirs.ai_import);
    if retired == Retired::Kept {
        eprintln!(
            "inferd: the old AI key registry in {} is left until accountd has copied it",
            dirs.old_keys.display()
        );
    }
    retired
}

/// [`retire`] over the three paths.
pub fn retire_at(registry_dir: &Path, audit: &Path, marker: &Path) -> Retired {
    let file = registry_dir.join("registry.json");
    let Ok(bytes) = std::fs::read(&file) else {
        return Retired::NothingThere;
    };
    match ImportMarker::read(marker) {
        Ok(Some(done)) if done.covers(&bytes) => {}
        _ => return Retired::Kept,
    }
    // Only a real directory, never a link to one, and only plain files in it.
    let Ok(meta) = std::fs::symlink_metadata(registry_dir) else {
        return Retired::NothingThere;
    };
    if !meta.is_dir() {
        return Retired::Kept;
    }
    let entries = match std::fs::read_dir(registry_dir) {
        Ok(entries) => entries,
        Err(_) => return Retired::Kept,
    };
    let plain =
        |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file());
    for entry in entries.flatten() {
        if plain(&entry.path()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::remove_dir(registry_dir);
    if plain(audit) {
        let _ = std::fs::remove_file(audit);
    }
    Retired::Removed
}

#[cfg(test)]
mod tests;
