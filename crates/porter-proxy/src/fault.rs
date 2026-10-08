//! Why a relay ended without finishing.

/// Why a relay ended without finishing. The host maps it to the account's state: `Refused` is
/// `NeedsReauth`, `Unreachable` is `Offline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelayFault {
    /// The server refused the credential.
    Refused,
    /// The server could not be reached.
    Unreachable,
    /// The TLS handshake or certificate check failed.
    Tls,
    /// The server (or, for HTTP, the app) did not speak the protocol, or offered no way to
    /// authenticate the credential's kind (a password with `LOGINDISABLED` and no TLS).
    Protocol,
    /// The app asked for an origin other than the endpoint's, or (HTTP) a path outside where
    /// the endpoint's grant reaches.
    ForeignOrigin,
}
