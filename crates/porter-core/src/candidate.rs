//! An account an app holds a grant for, as a query returns it.

use crate::account::AccountLabel;
use crate::capability::Capability;
use crate::id::{AccountId, GrantId, ProviderId};
use crate::offer::Subject;
use crate::restriction::Restriction;
use serde::{Deserialize, Serialize};

/// One granted account that meets a need: what an app renders and then uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The account.
    pub account: AccountId,
    /// Its label.
    pub label: AccountLabel,
    /// Its provider, for the mark.
    pub provider: ProviderId,
    /// The account, or the model of it, that fits.
    pub subject: Subject,
    /// The effective capability that fits.
    pub capability: Capability,
    /// What limits it, for the secondary line.
    pub restriction: Restriction,
    /// The grant the app holds for it; tokens are asked for by this.
    pub grant: GrantId,
}
