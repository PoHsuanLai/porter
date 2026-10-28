//! porter's pure accounts vocabulary: accounts and their endpoints, consent, and credentials'
//! filing. No runtime, no file system, no I/O: the file writer lives in `porter-fs`. What it is
//! built from lives below it, each in its own crate: ids, units, URLs, the clock and
//! percent-encoding in `porter-ids`, the capability vocabulary in `porter-capability`.

mod account;
mod agent_login;
mod ai_props;
mod app_id;
pub mod audit;
mod auth_kind;
mod candidate;
pub mod consent;
mod data_class;
mod endpoint;
mod error;
mod family;
mod helper_id;
mod identities;
mod identity;
mod ids;
mod launcher_session;
pub mod lending;
mod process_credential;
mod protocol;
mod provider_hint;
mod restriction;
pub mod scope;
mod secret;
mod space;
mod space_record;
pub mod store;
pub mod stream;
mod tailnet;
pub mod token;

pub use account::{Account, AccountLabel, AccountState, AgentState};
pub use agent_login::{LoginFault, LoginOutcome, LoginRequestId};
pub use ai_props::{Billing, Locality, PriceTable, Region, Tier};
pub use app_id::{AppId, AppLabel, AppName, Isolation};
pub use auth_kind::{AuthKind, SignInWay};
pub use candidate::Candidate;
pub use data_class::DataClass;
pub use endpoint::{
    EndpointFault, EndpointUrl, LoginName, PathReach, Reach, ReachMethods, RelayAuth, RelayPlan,
    ServerAddress, ServerAddressFault, ServiceEndpoint, Tls, server_address, server_address_shape,
};
pub use error::CoreError;
pub use family::Family;
pub use helper_id::HelperId;
pub use identities::{Identity, IdentityAddress, IdentityId, IdentityKind, IdentityName};
pub use identity::{CgroupPath, ClaimedId, PeerFacts, PeerIdentity, SandboxFacts, identity_of};
pub use ids::{AccountId, GrantId, KeyPermitId, ProviderId};
pub use launcher_session::LauncherSession;
pub use process_credential::ProcessCredentialId;
pub use protocol::EndpointProtocol;
pub use provider_hint::ProviderHint;
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
