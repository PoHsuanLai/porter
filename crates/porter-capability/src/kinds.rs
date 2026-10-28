//! The capability kinds, each written once. [`capability_kinds!`] is the one list: a row is the
//! kind's docs, its variant, the struct of its capability, the struct of its need (porter-match's)
//! and the name a person reads for it, with `ai` after the name for the kinds the AI broker
//! routes. Everything that has one case per kind is generated from it: [`Capability`],
//! [`CapabilityKind`] with its [`name`](CapabilityKind::name) and
//! [`is_ai`](CapabilityKind::is_ai), and the [`Service`] a kind is. porter-match expands the same
//! list into `Need`, so a new kind is one row here, its capability struct and its need struct.
//!
//! The serde forms are the bus and file forms (`kind = "mail"`, `{ "kind": "mail", "v": {..} }`);
//! a test pins them.

use crate::agent::AgentCap;
use crate::ai::{EmbedCap, ImageGenCap, LlmCap, RerankCap, SpeechCap};
use crate::cua::CuaCap;
use crate::mail::{IdentityCap, MailCap};
use crate::photos::PhotosCap;
use crate::pim::{NotesCap, PimCap};
use crate::storage::StorageCap;
use crate::sync_kinds::{KeyValueCap, PushCap};
use serde::{Deserialize, Serialize};

/// The list of capability kinds, handed to the macro named `$callback`, which sees every row as
/// `$(#[doc])* Variant(CapabilityStruct, NeedStruct, "Name" $(, ai)?),`. The need structs are
/// named for the expansion site: only porter-match, which defines them, expands them.
#[macro_export]
macro_rules! capability_kinds {
    ($callback:ident) => {
        $callback! {
            /// Who the account is.
            Identity(IdentityCap, IdentityNeed, "Account details"),
            /// A mailbox.
            Mail(MailCap, MailNeed, "Mail"),
            /// Calendars.
            Calendar(PimCap, PimNeed, "Calendar"),
            /// Address books.
            Contacts(PimCap, PimNeed, "Contacts"),
            /// Task lists.
            Tasks(PimCap, PimNeed, "Tasks"),
            /// Notes.
            Notes(NotesCap, NotesNeed, "Notes"),
            /// Files.
            Storage(StorageCap, StorageNeed, "Files"),
            /// A provider photo library.
            Photos(PhotosCap, PhotosNeed, "Photos"),
            /// A language model.
            Llm(LlmCap, LlmNeed, "Language model", ai),
            /// An embedding model.
            Embeddings(EmbedCap, EmbedNeed, "Search by meaning", ai),
            /// Speech in or out.
            Speech(SpeechCap, SpeechNeed, "Speech", ai),
            /// Image generation.
            ImageGen(ImageGenCap, ImageGenNeed, "Image generation", ai),
            /// Reranking.
            Rerank(RerankCap, RerankNeed, "Result ranking", ai),
            /// A model that operates a window from screenshots.
            ComputerUse(CuaCap, CuaNeed, "Operating windows", ai),
            /// Small synced items.
            KeyValue(KeyValueCap, KeyValueNeed, "Small synced items"),
            /// A push channel.
            Push(PushCap, PushNeed, "Notifications"),
            /// An external coding agent program that an account runs (family `acp_agent`).
            Agent(Box<AgentCap>, AgentNeed, "Assistant"),
        }
    };
}

/// Whether a row says `ai`.
#[doc(hidden)]
#[macro_export]
macro_rules! capability_kind_is_ai {
    () => {
        false
    };
    (ai) => {
        true
    };
}

/// What a kind is, as a type: its [`CapabilityKind`], the name a person reads and whether the AI
/// broker routes it. One zero-sized type per kind lives in [`service`] (the calendar, the contacts
/// and the tasks share one capability struct, so the struct cannot be the kind). porter-match's
/// `Asks` adds the need that asks for it.
pub trait Service {
    /// The kind.
    const KIND: CapabilityKind;
    /// What a person calls the service: the one name for the account sheet's services, Settings'
    /// switches and the noun in a grant's row ("Sync can use Files").
    const NAME: &'static str;
    /// Whether the AI broker routes this kind (its locality and billing matter).
    const AI: bool;
}

macro_rules! define_capabilities {
    ($($(#[$doc:meta])* $variant:ident($cap:ty, $need:ty, $name:literal $(, $ai:ident)?),)*) => {
        /// One thing an account (or one of its models) can do, with its parameters.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(tag = "kind", content = "v", rename_all = "snake_case")]
        #[non_exhaustive]
        pub enum Capability {
            $($(#[$doc])* $variant($cap),)*
        }

        impl Capability {
            /// Which kind this is.
            pub fn kind(&self) -> CapabilityKind {
                match self {
                    $(Capability::$variant(_) => CapabilityKind::$variant,)*
                }
            }
        }

        /// A capability's kind without its parameters: the unit of consent, toggles and limits.
        /// Its serde form is the stable slug used in files, on the bus and in the consent store.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[non_exhaustive]
        pub enum CapabilityKind {
            $($(#[$doc])* $variant,)*
        }

        impl CapabilityKind {
            /// Every kind, in the order of the list.
            pub const ALL: &'static [CapabilityKind] = &[$(CapabilityKind::$variant,)*];

            /// What a person calls the service ([`Service::NAME`]).
            pub fn name(self) -> &'static str {
                match self {
                    $(CapabilityKind::$variant => <service::$variant as Service>::NAME,)*
                }
            }

            /// Whether the AI broker routes this kind (its locality and billing matter).
            pub fn is_ai(self) -> bool {
                match self {
                    $(CapabilityKind::$variant => <service::$variant as Service>::AI,)*
                }
            }
        }

        /// One type per kind, each its [`Service`].
        pub mod service {
            use super::{CapabilityKind, Service};
            $(
                #[doc = concat!("The ", $name, " service.")]
                #[derive(Debug, Clone, Copy, PartialEq, Eq)]
                pub struct $variant;

                impl Service for $variant {
                    const KIND: CapabilityKind = CapabilityKind::$variant;
                    const NAME: &'static str = $name;
                    const AI: bool = $crate::capability_kind_is_ai!($($ai)?);
                }
            )*
        }
    };
}

capability_kinds!(define_capabilities);

#[cfg(test)]
mod tests;
