//! The protocol families as code (porter PLAN G6): [`DynProvider`], the object-safe twin of
//! `Provider`: a family is picked at run time from a provider file, so the host holds
//! `Box<dyn DynProvider>`, which is itself a `Provider`. accountd and an app hosting porter in
//! process (mailo) both build it, so a family is written once.
//!
//! This crate is the registry. The families that have a protocol of their own live in crates of
//! their own, each a driver over a sans-IO `*-proto` crate: `porter-family-oauth` (over
//! `oauth-probe-proto`), `porter-family-generic` (over `generic-proto`) and
//! `porter-family-nextcloud` (over `nextcloud-proto`); what they are written with is
//! `porter-family-kit`. Here are the small families (an API key, a coding agent that signs
//! itself in, Tailscale) and, with the `set` feature (every family a daemon maps), `set`: which
//! provider files a daemon serves and the family of each, shared by accountd, inferd and syncd.

#[cfg(feature = "agent_login")]
mod agent_login;
#[cfg(feature = "api_key")]
mod api_key;
mod dispatch;
#[cfg(feature = "api_key")]
mod key;
#[cfg(feature = "set")]
pub mod set;
#[cfg(feature = "tailnet")]
mod tailnet;

#[cfg(feature = "agent_login")]
pub use agent_login::{AgentLoginFlow, AgentLoginProvider, AgentLoginSession};
#[cfg(feature = "api_key")]
pub use api_key::{ApiKeyFlow, ApiKeyProvider, ApiKeySession};
pub use dispatch::{DynProvider, DynSession, DynSignIn};
#[cfg(feature = "tailnet")]
pub use tailnet::{TailnetFlow, TailnetProvider, TailnetSession};
