//! porter's pure vocabulary: accounts, the capability vocabulary and how a need meets an offer,
//! consent, credentials' filing, and the wire protocol. No I/O, no runtime, portable.

mod account;
mod ai_props;
mod app_id;
pub mod audit;
mod auth_kind;
mod candidate;
pub mod capability;
pub mod consent;
mod data_class;
mod effective;
mod endpoint;
mod error;
mod family;
#[cfg(test)]
mod fixtures;
mod id;
mod identity;
mod matching;
pub mod need;
mod offer;
mod restriction;
mod secret;
pub mod sheet;
mod space;
pub mod store;
pub mod stream;
mod token;
mod units;
mod weburl;
pub mod wire;

pub use account::{Account, AccountLabel, AccountState, AgentState};
pub use ai_props::{Billing, Locality, PriceTable, Region, Tier};
pub use app_id::{AppId, AppName, Isolation};
pub use auth_kind::AuthKind;
pub use candidate::Candidate;
pub use capability::{Capability, CapabilityKind};
pub use data_class::DataClass;
pub use effective::{KindToggle, Toggle, effective};
pub use endpoint::{
    EndpointFault, EndpointProtocol, EndpointUrl, LoginName, Origin, RelayAuth, RelayPlan,
    ServiceEndpoint, Tls, UrlScheme,
};
pub use error::CoreError;
pub use family::Family;
pub use id::{AccountId, GrantId, ModelId, ProviderId, is_id, object_segment};
pub use identity::{CgroupPath, ClaimedId, PeerFacts, PeerIdentity, SandboxFacts, identity_of};
pub use matching::{Match, Shortfall, matches};
pub use need::Need;
pub use offer::{AbsentReason, Claim, Offer, Provenance, Subject};
pub use restriction::{
    Limit, LimitReason, Restriction, TenantConsent, TokenLifetime, Verification,
};
pub use secret::{Credential, SecretKey, SecretPurpose, SecretText};
pub use space::{SpaceId, SpaceScope};
pub use token::{Audience, IssuedToken, TokenKind};
pub use units::{Bytes, Count, Dims, MicroUsd, Permille, Px, Tokens, UnixSeconds};
pub use weburl::WebUrl;
pub use wire::{AccountsReply, AccountsRequest};
