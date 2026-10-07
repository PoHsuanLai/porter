//! What an app asks for (design/31 §5.1): one variant per capability kind, every field a
//! minimum. Protocol fields (transport, wire, hashes) are the engine's business and are not
//! asked for.

mod agent;
mod ai;
mod data;

pub use agent::AgentNeed;
pub use ai::{CuaNeed, DimsNeed, EmbedNeed, ImageGenNeed, LlmNeed, RerankNeed, SpeechNeed};
pub use data::{
    IdentityNeed, KeyValueNeed, MailNeed, NotesNeed, PhotosNeed, PimNeed, PushNeed, StorageNeed,
};

use crate::capability::CapabilityKind;
use serde::{Deserialize, Serialize};

/// A capability query: "an account that can do at least this".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Need {
    /// Who the account is.
    Identity(IdentityNeed),
    /// A mailbox.
    Mail(MailNeed),
    /// Calendars.
    Calendar(PimNeed),
    /// Address books.
    Contacts(PimNeed),
    /// Task lists.
    Tasks(PimNeed),
    /// Notes.
    Notes(NotesNeed),
    /// Files.
    Storage(StorageNeed),
    /// A provider photo library.
    Photos(PhotosNeed),
    /// A language model.
    Llm(LlmNeed),
    /// An embedding model.
    Embeddings(EmbedNeed),
    /// Speech in or out.
    Speech(SpeechNeed),
    /// Image generation.
    ImageGen(ImageGenNeed),
    /// Reranking.
    Rerank(RerankNeed),
    /// A model that operates a window from screenshots.
    ComputerUse(CuaNeed),
    /// Small synced items.
    KeyValue(KeyValueNeed),
    /// A push channel.
    Push(PushNeed),
    /// An account that runs an agent program.
    Agent(AgentNeed),
}

impl Need {
    /// The kind asked for.
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Need::Identity(_) => CapabilityKind::Identity,
            Need::Mail(_) => CapabilityKind::Mail,
            Need::Calendar(_) => CapabilityKind::Calendar,
            Need::Contacts(_) => CapabilityKind::Contacts,
            Need::Tasks(_) => CapabilityKind::Tasks,
            Need::Notes(_) => CapabilityKind::Notes,
            Need::Storage(_) => CapabilityKind::Storage,
            Need::Photos(_) => CapabilityKind::Photos,
            Need::Llm(_) => CapabilityKind::Llm,
            Need::Embeddings(_) => CapabilityKind::Embeddings,
            Need::Speech(_) => CapabilityKind::Speech,
            Need::ImageGen(_) => CapabilityKind::ImageGen,
            Need::Rerank(_) => CapabilityKind::Rerank,
            Need::ComputerUse(_) => CapabilityKind::ComputerUse,
            Need::KeyValue(_) => CapabilityKind::KeyValue,
            Need::Push(_) => CapabilityKind::Push,
            Need::Agent(_) => CapabilityKind::Agent,
        }
    }
}
