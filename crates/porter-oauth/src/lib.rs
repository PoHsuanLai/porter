//! OAuth for the families (porter PLAN §2.6), from mailo's `oauth`, `signin`, `renewal` and
//! `loopback`: PKCE values (`pkce`), what a loopback redirect may be (`loopback`, with the
//! listener behind feature `io`), which client id this build presents (`registry`), when a
//! token is renewed (`renewal`), and the code exchange, refresh and revoke calls over the
//! [`porter_http::Http`] seam (`exchange`) and the authorize URLs (`authorize`). Randomness
//! and the clock are arguments; nothing here reads the environment.

mod authorize;
mod exchange;
mod form;
mod issuer;
mod loopback;
#[cfg(feature = "io")]
mod loopback_io;
mod pkce;
mod registry;
mod renewal;
#[cfg(test)]
mod scripted;

pub use authorize::{AuthorizeAsk, GooglePrompt, authorize_url, authorize_url_for, redeem_scope};
pub use exchange::{
    DEFAULT_EXPIRES_IN_SECONDS, ExchangeFailure, ExchangeFault, TokenResponse, exchange_code,
    exchange_code_scoped, exchange_code_scoped_detailed, refresh, refresh_scoped,
    refresh_scoped_detailed, revoke,
};
pub use issuer::{IssuerSays, SAYS_MAX_CHARS};
pub use loopback::{
    AuthCode, LoopbackFault, MAX_REDIRECT_BYTES, MAX_STRAY_REQUESTS, REDIRECT_WAIT_SECONDS,
    STRAY_READ_SECONDS, parse_redirect,
};
#[cfg(feature = "io")]
pub use loopback_io::{LoopbackServer, RedirectPath};
pub use pkce::{OAuthState, Pkce};
pub use registry::{ClientRegistry, ClientTraits, RegistryError, endpoints_of, from_mailo};
pub use renewal::{RENEW_MARGIN_SECONDS, RenewOutcome, is_due, renew};
