//! The capability vocabulary (design/31 §2): a closed set, versioned as a whole.

mod ai;
mod cua;
mod kind;
mod mail;
mod photos;
mod pim;
mod storage;
mod sync_kinds;
mod terms;

pub use ai::{
    EmbedCap, ImageGenCap, ImageMode, LanguageSet, LanguageTag, LlmCap, LlmFeature, LlmWire,
    Modality, RerankCap, SpeechCap, SpeechMode,
};
pub use cua::{CuaBatching, CuaCap, CuaEnv};
pub use kind::{CapabilityKind, VocabVersion};
pub use mail::{IdentityCap, LabelModel, MailCap, MailTransport};
pub use photos::{Albums, LibraryRead, PhotosCap};
pub use pim::{NotesCap, NotesTransport, PimCap, PimTransport};
pub use storage::{HashKind, StorageCap, StorageScope};
pub use sync_kinds::{KeyValueCap, PushCap, PushChannel};
pub use terms::{Access, Delta, Offered, QuotaReport};

use serde::{Deserialize, Serialize};

/// One thing an account (or one of its models) can do, with its parameters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Capability {
    /// Who the account is.
    Identity(IdentityCap),
    /// A mailbox.
    Mail(MailCap),
    /// Calendars.
    Calendar(PimCap),
    /// Address books.
    Contacts(PimCap),
    /// Task lists.
    Tasks(PimCap),
    /// Notes.
    Notes(NotesCap),
    /// Files.
    Storage(StorageCap),
    /// A provider photo library.
    Photos(PhotosCap),
    /// A language model.
    Llm(LlmCap),
    /// An embedding model.
    Embeddings(EmbedCap),
    /// Speech in or out.
    Speech(SpeechCap),
    /// Image generation.
    ImageGen(ImageGenCap),
    /// Reranking.
    Rerank(RerankCap),
    /// A model that operates a window from screenshots.
    ComputerUse(CuaCap),
    /// Small synced items.
    KeyValue(KeyValueCap),
    /// A push channel.
    Push(PushCap),
}

impl Capability {
    /// Which kind this is.
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Capability::Identity(_) => CapabilityKind::Identity,
            Capability::Mail(_) => CapabilityKind::Mail,
            Capability::Calendar(_) => CapabilityKind::Calendar,
            Capability::Contacts(_) => CapabilityKind::Contacts,
            Capability::Tasks(_) => CapabilityKind::Tasks,
            Capability::Notes(_) => CapabilityKind::Notes,
            Capability::Storage(_) => CapabilityKind::Storage,
            Capability::Photos(_) => CapabilityKind::Photos,
            Capability::Llm(_) => CapabilityKind::Llm,
            Capability::Embeddings(_) => CapabilityKind::Embeddings,
            Capability::Speech(_) => CapabilityKind::Speech,
            Capability::ImageGen(_) => CapabilityKind::ImageGen,
            Capability::Rerank(_) => CapabilityKind::Rerank,
            Capability::ComputerUse(_) => CapabilityKind::ComputerUse,
            Capability::KeyValue(_) => CapabilityKind::KeyValue,
            Capability::Push(_) => CapabilityKind::Push,
        }
    }
}
