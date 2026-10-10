//! An account an app holds a grant for, as a query returns it.

use crate::account::AccountLabel;
use crate::capability::Capability;
use crate::endpoint::ServiceEndpoint;
use crate::id::{AccountId, GrantId, ProviderId};
use crate::offer::Subject;
use crate::restriction::Restriction;
use serde::{Deserialize, Serialize};

/// One granted account that meets a need: what an app renders and then uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// The account's servers for the kind that fits (an IMAP and an SMTP host for mail, the
    /// WebDAV root for files), so a granted app learns where to connect. Empty for an account
    /// with none (a local runtime). On the bus this is the vardict key `endpoints`.
    pub endpoints: Vec<ServiceEndpoint>,
}

impl Candidate {
    /// An account that meets a need, with the grant the app holds for it and no servers named.
    pub fn new(
        account: AccountId,
        label: AccountLabel,
        provider: ProviderId,
        subject: Subject,
        capability: Capability,
        restriction: Restriction,
        grant: GrantId,
    ) -> Self {
        Self {
            account,
            label,
            provider,
            subject,
            capability,
            restriction,
            grant,
            endpoints: Vec::new(),
        }
    }

    /// The same candidate, naming the account's servers for the kind that fits.
    pub fn with_endpoints(mut self, endpoints: Vec<ServiceEndpoint>) -> Self {
        self.endpoints = endpoints;
        self
    }
}
