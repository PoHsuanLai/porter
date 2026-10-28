//! The protocol families as code (porter PLAN G6): one module and one cargo feature per family,
//! and [`DynProvider`], the object-safe twin of `Provider`: a family is picked at run time from a
//! provider file, so the host holds `Box<dyn DynProvider>`, which is itself a `Provider`.
//! accountd and an app hosting porter in process (mailo) both build it, so a family is written
//! once.
//!
//! The `set` feature (every family a daemon maps) adds `set`: which provider files a daemon
//! serves and the family of each, shared by accountd, inferd and syncd.
//!
//! OAuth is the `oauth` feature: one provider, sign-in and session for every issuer, the rules of
//! Google and Microsoft beside them. Google is the owner's own Google Cloud client, a sign-in that
//! says it needs a client id until one is registered (docs/google.md).

#[cfg(feature = "agent_login")]
mod agent_login;
#[cfg(feature = "api_key")]
mod api_key;
#[cfg(any(
    feature = "agent_login",
    feature = "api_key",
    feature = "generic",
    feature = "nextcloud",
    feature = "oauth",
    feature = "tailnet"
))]
mod conversation;
mod dispatch;
#[cfg(feature = "oauth")]
mod env_common;
#[cfg(feature = "oauth")]
mod extend;
#[cfg(feature = "generic")]
mod generic;
#[cfg(feature = "oauth")]
mod google;
#[cfg(any(feature = "nextcloud", feature = "generic"))]
mod io;
#[cfg(feature = "api_key")]
mod key;
#[cfg(feature = "oauth")]
mod microsoft;
#[cfg(feature = "nextcloud")]
mod nextcloud;
#[cfg(feature = "oauth")]
mod oauth;
#[cfg(any(feature = "nextcloud", feature = "generic"))]
mod password;
#[cfg(feature = "set")]
pub mod set;
#[cfg(feature = "tailnet")]
mod tailnet;

#[cfg(feature = "agent_login")]
pub use agent_login::{AgentLoginFlow, AgentLoginProvider, AgentLoginSession};
#[cfg(feature = "api_key")]
pub use api_key::{ApiKeyFlow, ApiKeyProvider, ApiKeySession};
#[cfg(any(
    feature = "agent_login",
    feature = "api_key",
    feature = "generic",
    feature = "nextcloud",
    feature = "oauth",
    feature = "tailnet"
))]
pub use conversation::{Conversation, Flow, Move};
pub use dispatch::{DynProvider, DynSession, DynSignIn};
#[cfg(feature = "oauth")]
pub use env_common::{ClientFiles, Clock, Random};
#[cfg(feature = "generic")]
pub use generic::{GenericFlow, GenericProvider, GenericSession};
#[cfg(feature = "oauth")]
pub use google::{Google, GoogleState};
#[cfg(any(feature = "nextcloud", feature = "generic"))]
pub use io::{Pacing, SharedDns};
#[cfg(feature = "oauth")]
pub use microsoft::{Microsoft, is_personal};
#[cfg(feature = "nextcloud")]
pub use nextcloud::{NextcloudFlow, NextcloudProvider, NextcloudSession};
#[cfg(feature = "oauth")]
pub use oauth::{
    Browser, BrowserFlow, BrowserIssuer, Cx, Grant, OAuthEnv, OAuthProvider, OAuthSession, Standard,
};
#[cfg(feature = "tailnet")]
pub use tailnet::{TailnetFlow, TailnetProvider, TailnetSession};
