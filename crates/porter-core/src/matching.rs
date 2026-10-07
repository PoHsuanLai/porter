//! Does an offer meet a need? The one place a query meets the vocabulary.

use crate::capability::Capability;
use crate::need::{DimsNeed, Need};
use crate::offer::{AbsentReason, Offer};
use serde::{Deserialize, Serialize};

/// The answer for one need against one offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Match {
    /// The offer meets every minimum.
    Fits,
    /// Same kind, but this field falls short (the first one, in the need's field order).
    Short(Shortfall),
    /// Same kind, absent, for this reason.
    Absent(AbsentReason),
    /// A different kind altogether.
    OtherKind,
}

/// The field of a need an offer fell short on, so the UI can say why (G9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shortfall {
    /// Less access than asked.
    Access,
    /// A weaker change feed than asked.
    Delta,
    /// No sending.
    Send,
    /// No profile.
    Profile,
    /// No verified address.
    VerifiedAddress,
    /// App folder only, and the whole store was asked.
    Scope,
    /// No quota report.
    Quota,
    /// Less of the photo library than asked (Google Photos: picker only).
    LibraryRead,
    /// No upload.
    Upload,
    /// Fewer albums than asked.
    Albums,
    /// No video.
    Video,
    /// A model feature is missing.
    Features,
    /// The context window is smaller.
    Context,
    /// The vector length differs.
    Dims,
    /// An input modality is missing.
    Modalities,
    /// A mode is missing.
    Modes,
    /// Images are smaller.
    MaxSide,
    /// Fewer documents per call.
    MaxDocs,
    /// An environment is missing.
    Environments,
    /// Items are smaller.
    MaxItem,
    /// A different agent program.
    Program,
    /// The agent does not speak a protocol asked for.
    Protocols,
    /// The agent cannot be pointed at another base URL.
    BaseUrl,
}

/// How `offer` answers `need`.
pub fn matches(need: &Need, offer: &Offer) -> Match {
    match offer {
        Offer::Absent { kind, reason } if *kind == need.kind() => Match::Absent(*reason),
        Offer::Absent { .. } => Match::OtherKind,
        Offer::Present(capability) => fit(need, capability),
    }
}

/// `Fits`, or the first failing check, in order.
fn first_short(checks: &[(bool, Shortfall)]) -> Match {
    checks
        .iter()
        .find(|(ok, _)| !ok)
        .map_or(Match::Fits, |(_, short)| Match::Short(*short))
}

fn fit(need: &Need, capability: &Capability) -> Match {
    use Shortfall as S;
    match (need, capability) {
        (Need::Identity(n), Capability::Identity(c)) => first_short(&[
            (c.profile >= n.profile, S::Profile),
            (c.verified_address >= n.verified_address, S::VerifiedAddress),
        ]),
        (Need::Mail(n), Capability::Mail(c)) => first_short(&[
            (c.access >= n.access, S::Access),
            (c.send >= n.send, S::Send),
            (c.delta >= n.delta, S::Delta),
        ]),
        (Need::Calendar(n), Capability::Calendar(c))
        | (Need::Contacts(n), Capability::Contacts(c))
        | (Need::Tasks(n), Capability::Tasks(c)) => first_short(&[
            (c.access >= n.access, S::Access),
            (c.delta >= n.delta, S::Delta),
        ]),
        (Need::Notes(n), Capability::Notes(c)) => first_short(&[
            (c.access >= n.access, S::Access),
            (c.delta >= n.delta, S::Delta),
        ]),
        (Need::Storage(n), Capability::Storage(c)) => first_short(&[
            (c.access >= n.access, S::Access),
            (c.delta >= n.delta, S::Delta),
            (c.scope >= n.scope, S::Scope),
            (c.quota >= n.quota, S::Quota),
        ]),
        (Need::Photos(n), Capability::Photos(c)) => first_short(&[
            (c.library_read >= n.library_read, S::LibraryRead),
            (c.upload >= n.upload, S::Upload),
            (c.albums >= n.albums, S::Albums),
            (c.video >= n.video, S::Video),
            (c.delta >= n.delta, S::Delta),
        ]),
        (Need::Llm(n), Capability::Llm(c)) => first_short(&[
            (n.features.is_subset(&c.features), S::Features),
            (c.context >= n.context, S::Context),
        ]),
        (Need::Embeddings(n), Capability::Embeddings(c)) => first_short(&[
            (
                matches!(n.dims, DimsNeed::Any) || n.dims == DimsNeed::Exactly(c.dims),
                S::Dims,
            ),
            (n.modalities.is_subset(&c.modalities), S::Modalities),
        ]),
        (Need::Speech(n), Capability::Speech(c)) => {
            first_short(&[(n.modes.is_subset(&c.modes), S::Modes)])
        }
        (Need::ImageGen(n), Capability::ImageGen(c)) => first_short(&[
            (n.modes.is_subset(&c.modes), S::Modes),
            (c.max_side >= n.max_side, S::MaxSide),
        ]),
        (Need::Rerank(n), Capability::Rerank(c)) => {
            first_short(&[(c.max_docs >= n.max_docs, S::MaxDocs)])
        }
        (Need::ComputerUse(n), Capability::ComputerUse(c)) => {
            first_short(&[(n.environments.is_subset(&c.environments), S::Environments)])
        }
        (Need::KeyValue(n), Capability::KeyValue(c)) => first_short(&[
            (c.delta >= n.delta, S::Delta),
            (c.max_item >= n.max_item, S::MaxItem),
        ]),
        (Need::Push(_), Capability::Push(_)) => Match::Fits,
        (Need::Agent(n), Capability::Agent(c)) => first_short(&[
            (n.program == c.program, S::Program),
            (n.protocols.is_subset(&c.protocols), S::Protocols),
            (c.base_url() >= n.base_url, S::BaseUrl),
        ]),
        _ => Match::OtherKind,
    }
}

#[cfg(test)]
mod tests;
