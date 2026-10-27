//! What an issuer says when it refuses: the OAuth error body (RFC 6749 section 5.2), read into
//! text safe to show. It names nothing else of this crate.

use porter_http::http::Response;

/// What an issuer wrote in its error body: the `error` code and the human `error_description`
/// (RFC 6749 section 5.2), for a host to show. Both are untrusted text from the network, so each
/// has its control characters stripped and is cut to [`SAYS_MAX_CHARS`] characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuerSays {
    /// The `error` code, e.g. `invalid_grant`.
    pub error: Option<String>,
    /// The `error_description`.
    pub description: Option<String>,
}

/// The most characters kept of each of an issuer's `error` and `error_description`.
pub const SAYS_MAX_CHARS: usize = 200;

impl IssuerSays {
    /// Whether the issuer refused because the person never gave the access asked for, not
    /// because the grant is gone: Google's `invalid_scope` (a refresh that names a scope the
    /// grant lacks), and Microsoft's `AADSTS65001` ("the user or administrator has not
    /// consented"), which it sends as `invalid_grant`. Signing in again as before would not
    /// help; asking for that access would.
    pub fn wants_consent(&self) -> bool {
        self.error.as_deref() == Some("invalid_scope")
            || self
                .description
                .as_deref()
                .is_some_and(|d| d.contains("AADSTS65001"))
    }
}

/// The `error` of an OAuth error body, if the body is one.
pub(crate) fn oauth_error(response: &Response<Vec<u8>>) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(response.body())
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

/// Untrusted issuer text made safe to show: control characters dropped, cut to the cap, and
/// `None` when nothing printable is left.
fn tidy(text: &str) -> Option<String> {
    let kept: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(SAYS_MAX_CHARS)
        .collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

/// What an OAuth error body says (`error`, `error_description`), tidied; `None` when the body is
/// not one or says nothing.
pub(crate) fn issuer_says(response: &Response<Vec<u8>>) -> Option<IssuerSays> {
    let body = serde_json::from_slice::<serde_json::Value>(response.body()).ok()?;
    let field = |name: &str| body.get(name)?.as_str().and_then(tidy);
    let says = IssuerSays {
        error: field("error"),
        description: field("error_description"),
    };
    (says.error.is_some() || says.description.is_some()).then_some(says)
}
