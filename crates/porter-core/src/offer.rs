//! What an account says it can do, and how that is known (design/31 §2.4).

use crate::capability::{Capability, CapabilityKind};
use crate::id::ModelId;
use serde::{Deserialize, Serialize};

/// A kind an account has, with its parameters, or lacks, with the reason the UI shows (G9).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Offer {
    /// The account can do this.
    Present(Capability),
    /// The account cannot, and why.
    Absent {
        /// The kind that is missing.
        kind: CapabilityKind,
        /// Why.
        reason: AbsentReason,
    },
}

impl Offer {
    /// The kind offered or missing.
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Offer::Present(capability) => capability.kind(),
            Offer::Absent { kind, .. } => *kind,
        }
    }
}

/// Why an account lacks a kind its provider could have had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsentReason {
    /// The provider offers no access to it at all (iCloud Drive, R6).
    ProviderOffersNone,
    /// The organisation's tenant requires admin consent (R13).
    TenantConsent,
    /// This build's OAuth client is not verified for the scope (R2, R4).
    UnverifiedBuild,
    /// The user turned it off for this account.
    TurnedOff,
    /// A probe found it missing on this server (a Nextcloud without the Notes app).
    NotOnServer,
}

/// What an offer is about: the account as a whole, or one model it serves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Subject {
    /// The account (every data kind).
    Account,
    /// One model of an AI account.
    Model(ModelId),
    /// One agent program an account can run (`AgentCap::program`), so an account that may run
    /// several keeps one claim for each.
    Agent(crate::capability::AgentProgram),
}

/// How a claim is known. Ordered by authority: a later variant corrects an earlier one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// The provider file says so.
    Declared,
    /// Our shipped model table says so (providers whose model list has no capability data).
    Curated,
    /// The protocol's own session answer said so (IMAP CAPABILITY, JMAP session, `/api/show`).
    Discovered,
    /// A small real call showed it (a Graph 403, a one-image vision probe).
    Probed,
}

/// One statement about what an account can do, from one source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Claim {
    /// What the claim is about.
    pub subject: Subject,
    /// The offer.
    pub offer: Offer,
    /// Where it comes from.
    pub provenance: Provenance,
}
