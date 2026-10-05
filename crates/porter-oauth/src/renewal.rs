//! When an access token is renewed: a little before it expires, so an app never receives one
//! that dies in flight.

use porter_core::UnixSeconds;

/// How long before expiry a token counts as due.
pub const RENEW_MARGIN_SECONDS: i64 = 60;

/// Whether a token needs renewing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renewal {
    /// Good for longer than the margin.
    Fresh,
    /// Expired or about to.
    Due,
}

/// Whether a token that expires at `expires` needs renewing at `now`.
pub fn renewal(expires: UnixSeconds, now: UnixSeconds) -> Renewal {
    match expires.0 - now.0 > RENEW_MARGIN_SECONDS {
        true => Renewal::Fresh,
        false => Renewal::Due,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_due_inside_the_margin() {
        const CASES: &[(&str, i64, Renewal)] = &[
            ("an hour left", 3600, Renewal::Fresh),
            ("just outside the margin", 61, Renewal::Fresh),
            ("on the margin", 60, Renewal::Due),
            ("expiring", 1, Renewal::Due),
            ("expired", -5, Renewal::Due),
        ];
        for (name, left, want) in CASES {
            assert_eq!(
                renewal(UnixSeconds(1_000 + left), UnixSeconds(1_000)),
                *want,
                "{name}"
            );
        }
    }
}
