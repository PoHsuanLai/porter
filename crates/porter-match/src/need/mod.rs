//! What an app asks for (design/31 §5.1): one variant per capability kind, every field a
//! minimum. Protocol fields (transport, wire, hashes) are the engine's business and are not
//! asked for.

mod agent;
mod ai;
mod data;

pub use agent::AgentNeed;
pub use ai::{
    CuaNeed, DimsNeed, EmbedNeed, ImageGenNeed, LlmNeed, RerankNeed, SpeechNeed, llm_need,
};
pub use data::{
    IdentityNeed, KeyValueNeed, MailNeed, NotesNeed, PhotosNeed, PimNeed, PushNeed, StorageNeed,
};

use porter_capability::{CapabilityKind, Service, StorageScope, service};
use serde::{Deserialize, Serialize};

macro_rules! define_needs {
    ($($(#[$doc:meta])* $variant:ident($cap:ty, $need:ty, $name:literal $(, $ai:ident)?),)*) => {
        /// A capability query: "an account that can do at least this".
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(tag = "kind", content = "v", rename_all = "snake_case")]
        #[non_exhaustive]
        pub enum Need {
            $($(#[$doc])* $variant($need),)*
        }

        impl Need {
            /// The kind asked for.
            pub fn kind(&self) -> CapabilityKind {
                match self {
                    $(Need::$variant(_) => CapabilityKind::$variant,)*
                }
            }

            /// How far a grant made for this need reaches: a Storage need's scope (the app
            /// folder, or the whole store), and the app folder for every other kind, which has
            /// no reach of its own.
            pub fn reach(&self) -> StorageScope {
                match self {
                    Need::Storage(storage) => storage.scope,
                    _ => StorageScope::AppFolder,
                }
            }
        }

        /// The need a service asks with, beside what the service is ([`Service`]).
        pub trait Asks: Service {
            /// The need struct of this kind.
            type Need;
        }

        $(
            impl Asks for service::$variant {
                type Need = $need;
            }
        )*
    };
}

porter_capability::capability_kinds!(define_needs);
