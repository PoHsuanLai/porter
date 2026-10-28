//! porter's pure vocabulary: accounts, the capability vocabulary and how a need meets an offer,
//! consent, credentials' filing, and the wire protocol. No runtime, no file system, no I/O: the
//! file writer lives in `porter-fs`. The ids, units, URLs, the clock and percent-encoding it is
//! built from are in `porter-ids`.

mod account;
mod agent_login;
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
mod helper_id;
mod identities;
mod identity;
mod ids;
mod launcher_session;
pub mod lending;
mod matching;
pub mod need;
mod offer;
mod process_credential;
mod protocol;
mod restriction;
pub mod scope;
mod secret;
pub mod sheet;
mod space;
mod space_record;
pub mod store;
pub mod stream;
mod tailnet;
pub mod token;
pub mod wire;

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
    EndpointFault, EndpointUrl, LoginName, PathReach, Reach, ReachMethods, RelayAuth, RelayPlan,
    ServerAddress, ServerAddressFault, ServiceEndpoint, Tls, server_address, server_address_shape,
};
pub use error::CoreError;
pub use family::Family;
pub use helper_id::HelperId;
pub use identities::{Identity, IdentityAddress, IdentityId, IdentityKind, IdentityName};
pub use identity::{CgroupPath, ClaimedId, PeerFacts, PeerIdentity, SandboxFacts, identity_of};
pub use ids::{AccountId, GrantId, KeyPermitId, ModelId, ProviderId};
pub use launcher_session::LauncherSession;
pub use matching::{Match, Shortfall, matches};
pub use need::Need;
pub use offer::{AbsentReason, Claim, Offer, Provenance, Subject};
pub use process_credential::ProcessCredentialId;
pub use protocol::EndpointProtocol;
pub use restriction::{
    Limit, LimitReason, ReauthReason, Restriction, SEVEN_DAYS, TenantConsent, TokenLifetime,
    Verification,
};
pub use scope::ScopeName;
pub use secret::{
    Credential, GrantedScopes, SecretKey, SecretPurpose, SecretText, write_secret,
    write_secret_option,
};
pub use space::{DesktopSpace, LocalSpace, SpaceId, SpaceKind, SpaceScope};
pub use space_record::{
    DesktopSpaceRecord, SPACE_LOOK_MAX_BYTES, SPACE_NAME_MAX_CHARS, SpaceChange, SpaceLook,
    SpaceName,
};
pub use tailnet::{Machine, MachineLink, MachineOwner, MachineSsh, NodeId};
pub use token::{Audience, IssuedToken, TokenKind};
pub use wire::{AccountsReply, AccountsRequest};
