//! Why a request produced no response.

/// Why a request produced no response. A response with an error status is a response, not this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    /// The server could not be reached (no route, refused, a name that does not resolve).
    #[error("unreachable")]
    Unreachable,
    /// The TLS handshake failed or the certificate was refused.
    #[error("tls failure")]
    Tls,
    /// No answer in time.
    #[error("timed out")]
    TimedOut,
    /// The response was longer than this client reads.
    #[error("response too large")]
    TooLarge,
    /// The server did not speak HTTP.
    #[error("malformed response")]
    Malformed,
    /// No connection was opened because the account has not given the access this client needs
    /// yet: the relay host that opens connections answered `NeedsConsent` (a whole drive, asked
    /// when first used). Trying again does not help until the person gives it.
    #[error("access not given yet")]
    NotGranted,
}
