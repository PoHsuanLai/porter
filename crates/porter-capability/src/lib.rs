//! The capability vocabulary (design/31 §2): a closed set, versioned as a whole, what an
//! account offers ([`Offer`], [`Claim`], with their [`Provenance`]) and the effective set once the
//! person's switches are applied ([`effective`]). Pure values; no I/O.

mod agent;
mod agent_login;
mod ai;
mod ai_props;
mod cua;
mod effective;
#[cfg(any(test, feature = "testing"))]
pub mod fixtures;
mod kinds;
mod mail;
mod model_id;
mod offer;
mod photos;
mod pim;
mod storage;
mod sync_kinds;
mod terms;
mod vocab;

pub use agent::{AgentCap, AgentProgram, AgentProtocol, EnvName};
pub use agent_login::{LoginFault, LoginOutcome, LoginRequestId};
pub use ai::{
    EmbedCap, EmbedPrompts, ImageGenCap, ImageMode, LanguageSet, LanguageTag, LlmCap, LlmFeature,
    LlmWire, Modality, PrefixText, RerankCap, SpeechCap, SpeechMode,
};
pub use ai_props::{Billing, Locality, Region, Tier};
pub use cua::{CuaBatching, CuaCap, CuaEnv};
pub use effective::{KindToggle, Toggle, effective};
pub use kinds::{Capability, CapabilityKind, Service, service};
pub use mail::{IdentityCap, LabelModel, MailCap, MailTransport};
pub use model_id::ModelId;
pub use offer::{AbsentReason, Claim, Offer, Provenance, Subject, offers};
pub use photos::{Albums, LibraryRead, PhotosCap};
pub use pim::{NotesCap, NotesTransport, PimCap, PimTransport};
pub use storage::{HashKind, StorageCap, StorageScope};
pub use sync_kinds::{KeyValueCap, PushCap, PushChannel};
pub use terms::{Access, Ask, Delta, present, reported};
pub use vocab::VocabVersion;
