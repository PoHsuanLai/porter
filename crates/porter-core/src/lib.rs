//! porter's pure vocabulary: accounts, the capability vocabulary and how a need meets an offer,
//! consent, credentials' filing, and the wire protocol. No runtime, no file system, no I/O: the
//! file writer lives in `porter-fs`. The time is one seam, [`clock`]: its [`clock::SystemClock`]
//! reads the wall clock only when a daemon calls it.

mod account;
mod agent_login;
mod ai_props;
mod app_id;
pub mod audit;
mod auth_kind;
mod candidate;
pub mod capability;
pub mod clock;
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
mod launcher_session;
pub mod lending;
mod matching;
pub mod need;
mod offer;
mod process_credential;
mod restriction;
mod secret;
pub mod sheet;
mod space;
mod space_record;
pub mod store;
pub mod stream;
mod tailnet;
mod token;
mod units;
mod weburl;
pub mod wire;
pub mod xdg;

pub use account::{Account, AccountLabel, AccountState, AgentState};
pub use agent_login::{LoginFault, LoginOutcome, LoginRequestId};
pub use ai_props::{Billing, Locality, PriceTable, Region, Tier};
pub use app_id::{AppId, AppLabel, AppName, Isolation};
pub use auth_kind::{AuthKind, SignInWay};
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
pub use launcher_session::LauncherSession;
pub use matching::{Match, Shortfall, matches};
pub use need::Need;
pub use offer::{AbsentReason, Claim, Offer, Provenance, Subject};
pub use process_credential::ProcessCredentialId;
pub use restriction::{
    Limit, LimitReason, ReauthReason, Restriction, SEVEN_DAYS, TenantConsent, TokenLifetime,
    Verification,
};
pub use secret::{Credential, SecretKey, SecretPurpose, SecretText};
pub use space::{DesktopSpace, LocalSpace, SpaceId, SpaceKind, SpaceScope};
pub use space_record::{
    DesktopSpaceRecord, SPACE_LOOK_MAX_BYTES, SPACE_NAME_MAX_CHARS, SpaceChange, SpaceLook,
    SpaceName,
};
pub use tailnet::{Machine, MachineOwner, NodeId};
pub use token::{Audience, IssuedToken, TokenKind};
pub use units::{Bytes, Count, Dims, MicroUsd, Permille, Px, Tokens, UnixSeconds};
pub use weburl::WebUrl;
pub use wire::{AccountsReply, AccountsRequest};
