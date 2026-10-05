//! PKCE (RFC 7636) and the `state` of one sign-in. The randomness is an argument, so a test
//! pins it and the daemon draws it from the system.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use porter_core::SecretText;

/// The challenge sent with the authorize request: the verifier's SHA-256, base64url.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeChallenge(pub String);

/// The `state` that ties a redirect to the sign-in that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthState(pub String);

/// One sign-in's PKCE verifier and state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    /// The verifier, kept until the code exchange; secret, so its `Debug` is redacted.
    pub verifier: SecretText,
    /// The state.
    pub state: OAuthState,
}

impl Pkce {
    /// A verifier of 43 characters from 32 random bytes, and a state from 16.
    pub fn from_random(verifier: [u8; 32], state: [u8; 16]) -> Self {
        Self {
            verifier: SecretText::new(URL_SAFE_NO_PAD.encode(verifier)),
            state: OAuthState(URL_SAFE_NO_PAD.encode(state)),
        }
    }

    /// The `S256` challenge for the verifier.
    pub fn challenge(&self) -> CodeChallenge {
        todo!(
            "base64url of SHA-256 over the verifier; needs a SHA-256 in quire's pinned block \
             (FINDINGS.md)"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verifier_is_43_url_safe_characters_and_never_shows() {
        let pkce = Pkce::from_random([7; 32], [9; 16]);
        assert_eq!(pkce.verifier.expose().len(), 43);
        assert!(
            pkce.verifier
                .expose()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(pkce.state.0.len(), 22);
        assert!(!format!("{pkce:?}").contains(pkce.verifier.expose()));
    }

    #[test]
    #[ignore = "W5a fills `challenge` (RFC 7636 appendix B vector)"]
    fn the_challenge_matches_the_rfc_vector() {
        // RFC 7636 appendix B: verifier dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk.
        let pkce = Pkce {
            verifier: SecretText::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            state: OAuthState("s".into()),
        };
        assert_eq!(
            pkce.challenge(),
            CodeChallenge("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into())
        );
    }
}
