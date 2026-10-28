//! What the accounts domain keeps between calls, beside the context it shares.

use crate::launcher::Roster;
use crate::own_machines::OwnMachines;
use porter_core::AccountId;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// The accounts domain's own state. The host sets the roster and the own machines once; the
/// reserved ids are the domain's own bookkeeping.
#[derive(Debug, Default)]
pub struct AccountsState {
    /// Which agent programs have a launcher, once the host says (accountd, when it serves).
    pub(crate) roster: OnceLock<Roster>,
    /// The person's computers on their tailnet, once the host says (inferd, which has a view of
    /// the tailnet): an AI server's address may be plain `http` to one of them.
    pub(crate) own_machines: OnceLock<OwnMachines>,
    /// Account ids an add has chosen and is still filing secrets for: not in the registry yet
    /// (the row is added only once its secrets are stored), and taken all the same.
    pub(crate) reserved: Mutex<HashSet<AccountId>>,
}

impl AccountsState {
    /// Tells the domain which agent programs have a launcher, so that adding an agent account
    /// with none ends in `SignInFault::NoLauncher` before anything is stored. Set once; a
    /// service that is never told (no launcher bus: an app hosting porter in process) adds
    /// agent accounts as it always did.
    pub fn set_launcher_roster(&self, roster: Roster) {
        let _ = self.roster.set(roster);
    }

    /// Tells the domain where the person's computers on their tailnet are listed, so that an AI
    /// server's address in an add sheet may be plain `http` to one of them. Set once; a service
    /// that is never told accepts plain `http` for this computer only.
    pub fn set_own_machines(&self, machines: OwnMachines) {
        let _ = self.own_machines.set(machines);
    }
}
