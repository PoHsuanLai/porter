//! OAuth for the families (porter PLAN §2.6), from mailo's `oauth`, `signin`, `renewal` and
//! `loopback`: PKCE values (`pkce`), what a loopback redirect may be (`loopback`, with the
//! listener behind feature `io`), which client id this build presents (`registry`), when a
//! token is renewed (`renewal`), and the code exchange, refresh and revoke calls over the
//! [`porter_http::Http`] seam (`exchange`), the device-code flow (`device`), the authorize URLs
//! (`authorize`) and OpenRouter's key mint (`mint`). Randomness
//! and the clock are arguments; nothing here reads the environment.

mod authorize;
mod device;
mod exchange;
mod form;
mod loopback;
#[cfg(feature = "io")]
mod loopback_io;
mod mint;
mod pkce;
mod registry;
mod renewal;
#[cfg(test)]
mod scripted;

pub use authorize::{authorize_url, openrouter_auth_url, redeem_scope};
pub use device::{
    DeviceCodeResponse, DeviceFault, DevicePoll, await_device, poll_device, request_device_code,
};
pub use exchange::{
    ExchangeFault, TokenResponse, exchange_code, exchange_code_scoped, refresh, refresh_scoped,
    revoke,
};
pub use loopback::{
    AuthCode, LoopbackFault, MAX_REDIRECT_BYTES, REDIRECT_WAIT_SECONDS, parse_redirect,
};
#[cfg(feature = "io")]
pub use loopback_io::LoopbackServer;
pub use mint::mint_key;
pub use pkce::{CodeChallenge, OAuthState, Pkce};
pub use registry::{ClientRegistry, RegistryError, clients_toml, endpoints_of, from_mailo};
pub use renewal::{RENEW_MARGIN_SECONDS, RenewOutcome, Renewal, renew, renewal};
