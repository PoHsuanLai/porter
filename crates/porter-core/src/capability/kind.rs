//! The capability kinds as bare names, and the vocabulary's version.

use serde::{Deserialize, Serialize};

/// The version of the whole vocabulary. Adding a kind or a field bumps it; the daemon and the
/// client crate release together, and an older client skips kinds it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VocabVersion(pub u16);

impl VocabVersion {
    /// The version this build speaks.
    pub const CURRENT: VocabVersion = VocabVersion(7);
}

/// A capability's kind without its parameters: the unit of consent, toggles and limits.
/// Its serde form is the stable slug used in files, on the bus and in the consent store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Who the account is.
    Identity,
    /// A mailbox.
    Mail,
    /// Calendars.
    Calendar,
    /// Address books.
    Contacts,
    /// Task lists.
    Tasks,
    /// Notes.
    Notes,
    /// Files.
    Storage,
    /// A provider photo library.
    Photos,
    /// A language model.
    Llm,
    /// An embedding model.
    Embeddings,
    /// Speech in or out.
    Speech,
    /// Image generation.
    ImageGen,
    /// Reranking.
    Rerank,
    /// A model that operates a window from screenshots.
    ComputerUse,
    /// Small synced items.
    KeyValue,
    /// A push channel.
    Push,
    /// An external coding agent program.
    Agent,
}

impl CapabilityKind {
    /// Whether the AI broker routes this kind (its locality and billing matter).
    pub fn is_ai(self) -> bool {
        matches!(
            self,
            CapabilityKind::Llm
                | CapabilityKind::Embeddings
                | CapabilityKind::Speech
                | CapabilityKind::ImageGen
                | CapabilityKind::Rerank
                | CapabilityKind::ComputerUse
        )
    }
}
