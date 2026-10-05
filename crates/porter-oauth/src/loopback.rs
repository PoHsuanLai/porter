//! What a loopback redirect may be, from mailo's rules: the listener is on 127.0.0.1 only, reads
//! at most 8 KiB, waits at most 300 s, accepts one request, and the `state` must be the one the
//! sign-in sent.

use crate::pkce::OAuthState;
use porter_core::SecretText;

/// The most bytes of a redirect request that are read.
pub const MAX_REDIRECT_BYTES: usize = 8 * 1024;
/// How long the listener waits for the person, in seconds.
pub const REDIRECT_WAIT_SECONDS: u64 = 300;

/// The authorization code a redirect carried; secret until exchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCode(pub SecretText);

/// Why a redirect was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoopbackFault {
    /// The `state` is not this sign-in's.
    #[error("wrong state")]
    WrongState,
    /// The request was longer than the cap.
    #[error("redirect too large")]
    Oversized,
    /// The request carried no code.
    #[error("no code")]
    NoCode,
    /// The issuer redirected with an `error`.
    #[error("the issuer refused: {0}")]
    Refused(String),
    /// Not an HTTP request line.
    #[error("not a redirect request")]
    Malformed,
}

/// The code in the redirect request `head` (through the blank line), if its `state` is
/// `expected`.
pub fn parse_redirect(head: &[u8], expected: &OAuthState) -> Result<AuthCode, LoopbackFault> {
    let _ = (head, expected);
    todo!(
        "check the length, read the request line, split and percent-decode the query, compare \
         `state`, return `code` or the issuer's `error`"
    )
}
