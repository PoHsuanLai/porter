//! The protocol families as code (porter PLAN G6): one module and one cargo feature per family,
//! and [`FamilyProvider`], the closed enum that dispatches the `Provider` seam over the ones
//! built. accountd and an app hosting porter in process (mailo) both build it, so a family is
//! written once. With no feature on, the set is empty and uninhabited.
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
#[cfg(feature = "openrouter")]
mod openrouter;
#[cfg(any(feature = "nextcloud", feature = "generic"))]
mod password;
#[cfg(feature = "openrouter")]
mod skeleton;

#[cfg(feature = "agent_login")]
pub use agent_login::{AgentLoginProvider, AgentLoginSession, AgentLoginSignIn};
#[cfg(feature = "api_key")]
pub use api_key::{ApiKeyProvider, ApiKeySession, ApiKeySignIn};
pub use dispatch::{FamilyProvider, FamilySession, FamilySignIn};
#[cfg(any(feature = "microsoft", feature = "google"))]
pub use env_common::ClientFiles;
#[cfg(feature = "generic")]
pub use generic::{GenericProvider, GenericSession, GenericSignIn};
#[cfg(feature = "google")]
pub use google::{
    GoogleEnv, GoogleProvider, GoogleSession, GoogleSignIn, ReauthReason, Sensitivity,
    TESTING_SIGN_IN_SECONDS, scope_sensitivity, scopes_of, testing_expiry_seam,
};
#[cfg(any(feature = "nextcloud", feature = "generic"))]
pub use io::{Pacing, SharedDns};
#[cfg(feature = "microsoft")]
pub use microsoft::{
    AccountClass, Clock, MicrosoftEnv, MicrosoftProvider, MicrosoftSession, MicrosoftSignIn,
    Random, SignInFlow, classify,
};
#[cfg(feature = "nextcloud")]
pub use nextcloud::{NextcloudProvider, NextcloudSession, NextcloudSignIn};
#[cfg(feature = "openrouter")]
pub use openrouter::{OpenRouterProvider, OpenRouterSession, OpenRouterSignIn};
