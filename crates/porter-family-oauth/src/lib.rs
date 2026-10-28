//! OAuth: one provider, sign-in and session for every issuer, the rules of Google and Microsoft
//! beside them. Google is the owner's own Google Cloud client, a sign-in that says it needs a
//! client id until one is registered (docs/google.md). What an issuer publishes is data
//! (porter-provider's `issuers.toml`); what a token shows is oauth-probe-proto's; what is left
//! here is the sending and the rules a row cannot say.

mod env_common;
mod extend;
mod google;
mod microsoft;
mod oauth;

pub use env_common::{ClientFiles, Clock, Random};
pub use google::{Google, GoogleState};
pub use microsoft::Microsoft;
pub use oauth::{
    Browser, BrowserFlow, BrowserIssuer, Cx, Grant, OAuthEnv, OAuthProvider, OAuthSession, Standard,
};
