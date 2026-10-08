//! The credentials handed to processes the agent launcher spawns (agent-session ask P2, lane
//! p2-handoff): what is held, and the files a `tmpfs_file` credential lives in.
//!
//! accountd keeps no copy of a key it hands out. A `memfd` credential is a sealed descriptor
//! that left in the reply, so accountd holds only the record (who was issued it, for which grant),
//! and cannot reach the copy the child has. A `tmpfs_file` credential is a 0600 file in a 0700
//! directory of its own under `$XDG_RUNTIME_DIR/porter/agent/<credential id>/key`; accountd
//! unlinks it when the credential ends, and clears the whole `agent` directory when it starts (a
//! file left by an accountd that died is a credential nobody can end).
//!
//! The bus-facing flow (who may issue, the audit lines, the signal) is in `handoff`.

use crate::keys::sealed_key;
use porter_core::audit::{CredentialEnd, Handoff};
use porter_core::wire::Refusal;
use porter_core::{AccountId, AppId, GrantId, ProcessCredentialId, SecretText};
use std::collections::BTreeMap;
use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// What the launcher is given to hand the child.
#[derive(Debug)]
pub(crate) enum Handle {
    /// A sealed memfd, read position at the start.
    Fd(OwnedFd),
    /// The path of a 0600 file.
    Path(PathBuf),
}

/// One credential accountd handed out and has not ended.
#[derive(Debug)]
pub(crate) struct Held {
    /// The launcher's connection (unique name) it was issued to: the only one that may end it,
    /// and the only one told when accountd ends it.
    pub(crate) owner: String,
    /// The app the grant is for (`org.quire.Agent.<program>`).
    pub(crate) audience: AppId,
    /// The account whose key it is.
    pub(crate) account: AccountId,
    /// The grant it was issued under.
    pub(crate) grant: GrantId,
    /// The directory of its file, for a `tmpfs_file` credential.
    dir: Option<PathBuf>,
}

impl Held {
    /// Removes the credential's file and directory, if it has them.
    fn erase(&self) {
        let Some(dir) = &self.dir else { return };
        for result in [
            std::fs::remove_file(dir.join(KEY_FILE)),
            std::fs::remove_dir(dir),
        ] {
            match result {
                Ok(()) => {}
                Err(why) if why.kind() == io::ErrorKind::NotFound => {}
                Err(why) => eprintln!("accountd: process credential: {}: {why}", dir.display()),
            }
        }
    }
}

/// The name of the file in a credential's directory.
const KEY_FILE: &str = "key";

/// A request for a credential, checked already: whose, for what, by which way.
#[derive(Debug)]
pub(crate) struct Issue<'a> {
    pub(crate) owner: &'a str,
    pub(crate) audience: &'a AppId,
    pub(crate) account: &'a AccountId,
    pub(crate) grant: &'a GrantId,
    pub(crate) key: &'a SecretText,
    pub(crate) handoff: Handoff,
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    held: BTreeMap<ProcessCredentialId, Held>,
}

/// The credentials in force.
#[derive(Debug)]
pub(crate) struct Credentials {
    /// `$XDG_RUNTIME_DIR/porter/agent`; none refuses `tmpfs_file` as unavailable.
    root: Option<PathBuf>,
    state: Mutex<State>,
}

fn locked(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // Every critical section is a plain data update.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Credentials {
    /// Credentials whose files live under `runtime_dir` (`$XDG_RUNTIME_DIR`), after clearing what
    /// an earlier accountd left there.
    pub(crate) fn new(runtime_dir: Option<&Path>) -> Self {
        let root = runtime_dir.map(|dir| dir.join("porter").join("agent"));
        if let Some(root) = &root
            && let Err(why) = std::fs::remove_dir_all(root)
            && why.kind() != io::ErrorKind::NotFound
        {
            eprintln!("accountd: process credential: {}: {why}", root.display());
        }
        Self {
            root,
            state: Mutex::default(),
        }
    }

    /// Makes a credential of `issue.key` by the way `issue.handoff` names, and records it.
    /// `Unavailable` when the descriptor or the file cannot be made (the cause goes to standard
    /// error, never the key).
    pub(crate) fn issue(
        &self,
        issue: &Issue<'_>,
    ) -> Result<(ProcessCredentialId, Handle), Refusal> {
        let id = self.next_id()?;
        let unavailable = |why: io::Error| {
            eprintln!("accountd: process credential {id}: {why}");
            Refusal::Unavailable
        };
        let (handle, dir) = match issue.handoff {
            Handoff::Memfd => {
                let fd = sealed_key(issue.key.expose()).map_err(unavailable)?;
                (Handle::Fd(fd), None)
            }
            Handoff::TmpfsFile => {
                let root = self.root.as_deref().ok_or(Refusal::Unavailable)?;
                let (path, dir) =
                    write_key_file(root, &id, issue.key.expose()).map_err(unavailable)?;
                (Handle::Path(path), Some(dir))
            }
        };
        locked(&self.state).held.insert(
            id.clone(),
            Held {
                owner: issue.owner.to_owned(),
                audience: issue.audience.clone(),
                account: issue.account.clone(),
                grant: issue.grant.clone(),
                dir,
            },
        );
        Ok((id, handle))
    }

    fn next_id(&self) -> Result<ProcessCredentialId, Refusal> {
        let mut state = locked(&self.state);
        state.next += 1;
        // "cred-<n>" is always an id; the error arm is not reachable.
        ProcessCredentialId::parse(&format!("cred-{}", state.next))
            .map_err(|_| Refusal::Unavailable)
    }

    /// Ends `id` for the connection `owner` that was issued it: `note` is told (the audit line),
    /// then its file goes. Any other connection, or an id that is not in force, is `None` and
    /// nothing is told.
    pub(crate) fn take(
        &self,
        owner: &str,
        id: &ProcessCredentialId,
        note: impl FnOnce(&Held),
    ) -> Option<Held> {
        let held = {
            let mut state = locked(&self.state);
            match state.held.get(id) {
                Some(held) if held.owner == owner => state.held.remove(id),
                _ => None,
            }
        }?;
        note(&held);
        held.erase();
        Some(held)
    }

    /// Ends every credential `why` gives an end for: `note` is told of each (the audit line),
    /// then its file goes, so whoever sees a file gone finds the line that says why. What ended,
    /// with the reason each did.
    pub(crate) fn end_where(
        &self,
        why: impl Fn(&Held) -> Option<CredentialEnd>,
        note: impl Fn(&Held, CredentialEnd),
    ) -> Vec<(ProcessCredentialId, Held, CredentialEnd)> {
        let ended: Vec<(ProcessCredentialId, Held, CredentialEnd)> = {
            let mut state = locked(&self.state);
            let ending: Vec<(ProcessCredentialId, CredentialEnd)> = state
                .held
                .iter()
                .filter_map(|(id, held)| why(held).map(|reason| (id.clone(), reason)))
                .collect();
            ending
                .into_iter()
                .filter_map(|(id, reason)| {
                    let held = state.held.remove(&id)?;
                    Some((id, held, reason))
                })
                .collect()
        };
        for (_, held, reason) in &ended {
            note(held, *reason);
            held.erase();
        }
        ended
    }
}

#[cfg(test)]
mod tests;

/// Creates `<root>/<id>/key` holding `key`: the directories 0700, the file 0600, the file new
/// (it is never opened over an existing one or through a link). Returns the file's path and its
/// directory. Nothing is left behind when it fails.
fn write_key_file(
    root: &Path,
    id: &ProcessCredentialId,
    key: &str,
) -> io::Result<(PathBuf, PathBuf)> {
    DirBuilder::new().recursive(true).mode(0o700).create(root)?;
    // Whoever made `root` before us may have left it open.
    std::fs::set_permissions(root, Permissions::from_mode(0o700))?;
    let dir = root.join(id.as_str());
    DirBuilder::new().mode(0o700).create(&dir)?;
    let path = dir.join(KEY_FILE);
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| file.write_all(key.as_bytes()));
    match written {
        Ok(()) => Ok((path, dir)),
        Err(why) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(why)
        }
    }
}
