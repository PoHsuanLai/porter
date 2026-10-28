//! The protocol families as code (porter PLAN G6): one module and one cargo feature per family,
//! and [`DynProvider`], the object-safe twin of `Provider`: a family is picked at run time from a
//! provider file, so the host holds `Box<dyn DynProvider>`, which is itself a `Provider`.
//! accountd and an app hosting porter in process (mailo) both build it, so a family is written
//! once.
//!
//! The `set` feature (every family a daemon maps) adds `set`: which provider files a daemon
//! serves and the family of each, shared by accountd, inferd and syncd.
//!
//! Google is the `google` feature: the owner's own Google Cloud client, a sign-in that says it
//! needs a client id until one is registered (docs/google.md).

#[cfg(feature = "agent_login")]
mod agent_login;
#[cfg(feature = "api_key")]
mod api_key;
mod dispatch;
#[cfg(any(feature = "microsoft", feature = "google"))]
mod env_common;
#[cfg(any(feature = "microsoft", feature = "google"))]
mod extend;
#[cfg(feature = "generic")]
mod generic;
#[cfg(feature = "google")]
mod google;
#[cfg(any(feature = "nextcloud", feature = "generic"))]
mod io;
#[cfg(feature = "api_key")]
mod key;
#[cfg(feature = "microsoft")]
mod microsoft;
#[cfg(feature = "nextcloud")]
mod nextcloud;
#[cfg(any(feature = "nextcloud", feature = "generic"))]
mod password;
#[cfg(feature = "set")]
pub mod set;
#[cfg(feature = "tailnet")]
mod tailnet;

#[cfg(feature = "agent_login")]
pub use agent_login::{AgentLoginProvider, AgentLoginSession, AgentLoginSignIn};
#[cfg(feature = "api_key")]
pub use api_key::{ApiKeyProvider, ApiKeySession, ApiKeySignIn};
pub use dispatch::{DynProvider, DynSession, DynSignIn};
#[cfg(any(feature = "microsoft", feature = "google"))]
pub use env_common::ClientFiles;
#[cfg(feature = "generic")]
pub use generic::{GenericProvider, GenericSession, GenericSignIn};
#[cfg(feature = "google")]
pub use google::{
    GoogleEnv, GoogleProvider, GoogleSession, GoogleSignIn, Sensitivity, on_use_scopes_of,
    scope_sensitivity, scopes_of,
};
#[cfg(any(feature = "nextcloud", feature = "generic"))]
pub use io::{Pacing, SharedDns};
#[cfg(feature = "microsoft")]
pub use microsoft::{
    Clock, MicrosoftEnv, MicrosoftProvider, MicrosoftSession, MicrosoftSignIn, Random, is_personal,
};
#[cfg(feature = "nextcloud")]
pub use nextcloud::{NextcloudProvider, NextcloudSession, NextcloudSignIn};
#[cfg(feature = "tailnet")]
pub use tailnet::{TailnetProvider, TailnetSession, TailnetSignIn};
