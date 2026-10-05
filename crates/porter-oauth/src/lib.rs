//! OAuth for the families (porter PLAN §2.6), from mailo's `oauth`, `signin`, `renewal` and
//! `loopback`: PKCE values (`pkce`), what a loopback redirect may be (`loopback`, with the
//! listener behind feature `io`), which client id this build presents (`registry`), when a
//! token is renewed (`renewal`), and the code exchange, refresh and revoke calls over the
//! [`porter_http::Http`] seam (`exchange`), plus the device-code answer (`device`). Randomness
//! and the clock are arguments; nothing here reads the environment.

mod device;
mod exchange;
mod loopback;
#[cfg(feature = "io")]
mod loopback_io;
mod pkce;
mod registry;
mod renewal;

pub use device::DeviceCodeResponse;
pub use exchange::{ExchangeFault, TokenResponse, exchange_code, refresh, revoke};
pub use loopback::{
    AuthCode, LoopbackFault, MAX_REDIRECT_BYTES, REDIRECT_WAIT_SECONDS, parse_redirect,
};
#[cfg(feature = "io")]
pub use loopback_io::LoopbackServer;
pub use pkce::{CodeChallenge, OAuthState, Pkce};
pub use registry::ClientRegistry;
pub use renewal::{RENEW_MARGIN_SECONDS, Renewal, renewal};
