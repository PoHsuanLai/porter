//! The protocol families as code (porter PLAN G6): one module and one cargo feature per family,
//! and [`FamilyProvider`], the closed enum that dispatches the `Provider` seam over the ones
//! built. accountd and an app hosting porter in process (mailo) both build it, so a family is
//! written once. With no feature on, the set is empty and uninhabited.
//!
//! Google is not here: the owner deferred it (FINDINGS.md), so `providers/google.toml` ships as
//! a file and has no family code behind it.

#[cfg(feature = "api_key")]
mod api_key;
mod dispatch;
#[cfg(feature = "generic")]
mod generic;
#[cfg(feature = "microsoft")]
mod microsoft;
#[cfg(feature = "nextcloud")]
mod nextcloud;
#[cfg(feature = "openrouter")]
mod openrouter;
#[cfg(any(
    feature = "nextcloud",
    feature = "generic",
    feature = "microsoft",
    feature = "api_key",
    feature = "openrouter"
))]
mod skeleton;

#[cfg(feature = "api_key")]
pub use api_key::{ApiKeyProvider, ApiKeySession, ApiKeySignIn};
pub use dispatch::{FamilyProvider, FamilySession, FamilySignIn};
#[cfg(feature = "generic")]
pub use generic::{GenericProvider, GenericSession, GenericSignIn};
#[cfg(feature = "microsoft")]
pub use microsoft::{MicrosoftProvider, MicrosoftSession, MicrosoftSignIn};
#[cfg(feature = "nextcloud")]
pub use nextcloud::{NextcloudProvider, NextcloudSession, NextcloudSignIn};
#[cfg(feature = "openrouter")]
pub use openrouter::{OpenRouterProvider, OpenRouterSession, OpenRouterSignIn};
