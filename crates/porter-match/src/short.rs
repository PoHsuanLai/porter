//! The ways an offer can fall short of a need, and the check each need makes of its own kind of
//! capability.

use serde::{Deserialize, Serialize};

/// The field of a need an offer fell short on, so the UI can say why (G9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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

/// How one need meets one kind of capability: the first field of the offer that falls short, in
/// the need's field order, or `None` when every minimum is met. The need of each kind implements
/// it for its capability; [`matches`](crate::matches) picks the pair.
pub trait Wants<C> {
    /// The first field of `offer` that falls short of this need.
    fn short_of(&self, offer: &C) -> Option<Shortfall>;
}

/// The first failing check, in order.
pub(crate) fn first_short(checks: &[(bool, Shortfall)]) -> Option<Shortfall> {
    checks.iter().find(|(ok, _)| !ok).map(|(_, short)| *short)
}
