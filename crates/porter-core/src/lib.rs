//! porter's pure accounts vocabulary: accounts and their endpoints, consent, and credentials'
//! filing. No runtime, no file system, no I/O: the file writer lives in `porter-fs`. What it is
//! built from lives below it, each in its own crate: ids, units, URLs, the clock and
//! percent-encoding in `porter-ids`, the capability vocabulary in `porter-capability`.

mod account;
mod app_id;
pub mod audit;
mod auth_kind;
mod candidate;
pub mod consent;
mod data_class;
mod endpoint;
mod error;
mod family;
mod grant_key;
mod helper_id;
mod identities;
mod identity;
mod ids;
mod launcher_session;
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
pub mod token;

pub use account::{Account, AccountLabel, AccountState, AgentState};
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
pub use grant_key::{GrantKey, decide_key};
pub use helper_id::HelperId;
pub use identities::{Identity, IdentityAddress, IdentityId, IdentityKind, IdentityName};
pub use identity::{CgroupPath, ClaimedId, PeerFacts, PeerIdentity, SandboxFacts, identity_of};
pub use ids::{AccountId, GrantId, ProviderId};
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
pub use token::{Audience, IssuedToken, TokenKind};
