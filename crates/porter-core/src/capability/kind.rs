//! The capability kinds as bare names, and the vocabulary's version.

use serde::{Deserialize, Serialize};

/// The version of the whole vocabulary. Adding a kind or a field bumps it; the daemon and the
/// client crate release together, and an older client skips kinds it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VocabVersion(pub u16);

impl VocabVersion {
    /// The version this build speaks.
    pub const CURRENT: VocabVersion = VocabVersion(12);
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

    /// The kind as a person reads it: the one source for the account sheet's service names,
    /// Settings' Services switches and the noun in a grant's row ("Sync can use Files"). No
    /// wildcard arm, so a new kind must be named here.
    pub fn display_name(self) -> &'static str {
        match self {
            CapabilityKind::Identity => "Account details",
            CapabilityKind::Mail => "Mail",
            CapabilityKind::Calendar => "Calendar",
            CapabilityKind::Contacts => "Contacts",
            CapabilityKind::Tasks => "Tasks",
            CapabilityKind::Notes => "Notes",
            CapabilityKind::Storage => "Files",
            CapabilityKind::Photos => "Photos",
            CapabilityKind::Llm => "Language model",
            CapabilityKind::Embeddings => "Search by meaning",
            CapabilityKind::Speech => "Speech",
            CapabilityKind::ImageGen => "Image generation",
            CapabilityKind::Rerank => "Result ranking",
            CapabilityKind::ComputerUse => "Operating windows",
            CapabilityKind::KeyValue => "Small synced items",
            CapabilityKind::Push => "Notifications",
            CapabilityKind::Agent => "Assistant",
        }
    }
}
