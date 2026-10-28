//! The credentials handed to processes the agent launcher spawns: what is held.
//!
//! accountd keeps no copy of a key it hands out. A credential is a sealed memfd that left in the
//! reply, so accountd holds only the record (who was issued it, under which permit) and cannot
//! reach the copy the child has. There is no file: nothing of a key is written under a runtime
//! directory.
//!
//! The bus-facing flow (who may issue, the audit lines, the signal) is in `shared`.

use porter_core::ProcessCredentialId;
use porter_core::audit::CredentialEnd;
use porter_core::{AccountId, GrantId};
use porter_core::{AppId, SecretText};
use porter_wire::Refusal;
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The prefix of a credential's id.
const ID_PREFIX: &str = "cred-";

/// One credential accountd handed out and has not ended.
#[derive(Debug)]
pub(crate) struct Held {
    /// The launcher's connection (unique name) it was issued to: the only one that may end it,
    /// and the only one told when accountd ends it.
    pub(crate) owner: String,
    /// The app the permit is for (`org.quire.Agent.<program>`).
    pub(crate) audience: AppId,
    /// The account whose key it is.
    pub(crate) account: AccountId,
    /// The permit it was issued under (a grant of the hosted service).
    pub(crate) grant: GrantId,
}

/// A request for a credential, checked already: whose, for what.
#[derive(Debug)]
pub(crate) struct Issue<'a> {
    pub(crate) owner: &'a str,
    pub(crate) audience: &'a AppId,
    pub(crate) account: &'a AccountId,
    pub(crate) grant: &'a GrantId,
    pub(crate) key: &'a SecretText,
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    held: BTreeMap<ProcessCredentialId, Held>,
}

/// The credentials in force.
#[derive(Debug, Default)]
pub(crate) struct Credentials {
    state: Mutex<State>,
}

fn locked(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // Every critical section is a plain data update.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Credentials {
    /// Makes a sealed memfd of `issue.key`, and records the credential. `Unavailable` when the
    /// descriptor cannot be made (the cause goes to standard error, never the key).
    pub(crate) fn issue(
        &self,
        issue: &Issue<'_>,
    ) -> Result<(ProcessCredentialId, OwnedFd), Refusal> {
        let id = self.next_id()?;
        let fd = porter_secrets::sealed_key(issue.key.expose()).map_err(|why| {
            eprintln!("accountd: process credential {id}: {why}");
            Refusal::Unavailable
        })?;
        locked(&self.state).held.insert(
            id.clone(),
            Held {
                owner: issue.owner.to_owned(),
                audience: issue.audience.clone(),
                account: issue.account.clone(),
                grant: issue.grant.clone(),
            },
        );
        Ok((id, fd))
    }

    fn next_id(&self) -> Result<ProcessCredentialId, Refusal> {
        let mut state = locked(&self.state);
        state.next += 1;
        // "cred-<n>" is always an id; the error arm is not reachable.
        ProcessCredentialId::parse(&format!("{ID_PREFIX}{}", state.next))
            .map_err(|_| Refusal::Unavailable)
    }

    /// Ends `id` for the connection `owner` that was issued it: `note` is told (the audit line).
    /// Any other connection, or an id that is not in force, is `None` and nothing is told.
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
        Some(held)
    }

    /// Ends every credential `why` gives an end for: `note` is told of each (the audit line).
    /// What ended, with the reason each did.
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
        }
        ended
    }
}

#[cfg(test)]
mod tests;
