//! PKCE (RFC 7636) and the `state` of one sign-in. The randomness is an argument, so a test
//! pins it and the daemon draws it from the system.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use porter_core::SecretText;
use sha2::{Digest, Sha256};

/// The challenge sent with the authorize request: the verifier's SHA-256, base64url.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeChallenge(pub String);

/// The `state` that ties a redirect to the sign-in that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthState(pub String);

impl OAuthState {
    /// Whether `presented` is this state. Compared without an early exit: the value is a secret
    /// and a timing oracle on it costs an attacker nothing (ported from mailo's `Pending::accepts`,
    /// `~/mailo/crates/mail-runtime/src/oauth.rs`).
    pub fn accepts(&self, presented: &str) -> bool {
        let (ours, theirs) = (self.0.as_bytes(), presented.as_bytes());
        ours.len() == theirs.len()
            && ours
                .iter()
                .zip(theirs)
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }
}

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
        CodeChallenge(URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.expose().as_bytes())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_accepts_only_itself() {
        let state = OAuthState("abc".into());
        assert!(state.accepts("abc"));
        for wrong in ["", "abd", "ab", "abcd", "ABC"] {
            assert!(!state.accepts(wrong), "{wrong}");
        }
    }

    #[test]
    fn two_sign_ins_never_share_a_verifier_or_state() {
        let (a, b) = (
            Pkce::from_random([1; 32], [2; 16]),
            Pkce::from_random([3; 32], [4; 16]),
        );
        assert_ne!(a.verifier.expose(), b.verifier.expose());
        assert_ne!(a.state, b.state);
    }

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
