//! Why a Google account has to be signed in again, in words the sheet can show, and the seven
//! day rule behind the one that is Google's doing.
//!
//! An OAuth client whose consent screen is in *testing* (the state every client the owner
//! registers starts in, and stays in until Google verifies it) gets refresh tokens that Google
//! ends after seven days (design/31 R5). The refusal looks like any other `invalid_grant`, so the
//! family says which it is from the client's row (`testing = true`) and, where it knows when the
//! person signed in, from the age of the sign-in.

use porter_core::UnixSeconds;
use porter_oauth::AppReview;

/// Seconds a refresh token of a client in testing lasts.
pub const TESTING_SIGN_IN_SECONDS: i64 = 7 * 24 * 60 * 60;

/// A refusal this young cannot be Google's seven days: the person ended it (or the password
/// changed). A day short of the seven is allowed for clocks and for how Google counts.
const TESTING_REFUSAL_FROM_SECONDS: i64 = 6 * 24 * 60 * 60;

/// Why Google would not renew a sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReauthReason {
    /// The client is in testing, and Google signs its accounts out every seven days.
    TestingExpiry,
    /// Google no longer accepts the sign-in: the person removed the app from their Google
    /// account, or changed the password.
    Revoked,
}

impl ReauthReason {
    /// The reason in plain words, for the sheet.
    pub fn plain(self) -> &'static str {
        match self {
            ReauthReason::TestingExpiry => {
                "Google signs this app out every 7 days until it is verified."
            }
            ReauthReason::Revoked => {
                "Google no longer accepts this sign-in. It may have been removed from your Google account."
            }
        }
    }

    /// What a refused refresh means for a client with `review`, given when the person signed in
    /// when that is known. A client in testing whose sign-in is old, or of unknown age, is the
    /// seven day rule; a young sign-in was ended by someone.
    pub fn of_refusal(review: AppReview, signed_in: Option<UnixSeconds>, now: UnixSeconds) -> Self {
        let aged = signed_in.is_none_or(|at| now.0 - at.0 >= TESTING_REFUSAL_FROM_SECONDS);
        match (review, aged) {
            (AppReview::Testing, true) => ReauthReason::TestingExpiry,
            _ => ReauthReason::Revoked,
        }
    }
}

/// Test seam: the day a sign-in made at `signed_in` through a client in testing stops working.
/// The product reads the reason ([`ReauthReason`]), not this date; the tests pin the figure.
pub fn testing_expiry_seam(signed_in: UnixSeconds) -> UnixSeconds {
    UnixSeconds(signed_in.0 + TESTING_SIGN_IN_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_the_seven_day_rule_only_for_a_client_in_testing_and_old_enough() {
        const DAY: i64 = 86_400;
        let now = UnixSeconds(100 * DAY);
        let at = |days_ago: i64| Some(UnixSeconds(now.0 - days_ago * DAY));
        const T: AppReview = AppReview::Testing;
        const V: AppReview = AppReview::Verified;
        let cases = [
            ("testing, a week old", T, at(7), ReauthReason::TestingExpiry),
            (
                "testing, six and a half",
                T,
                at(6),
                ReauthReason::TestingExpiry,
            ),
            ("testing, a day old", T, at(1), ReauthReason::Revoked),
            ("testing, age unknown", T, None, ReauthReason::TestingExpiry),
            ("verified, a week old", V, at(7), ReauthReason::Revoked),
            ("verified, age unknown", V, None, ReauthReason::Revoked),
        ];
        for (name, review, signed_in, want) in cases {
            assert_eq!(
                ReauthReason::of_refusal(review, signed_in, now),
                want,
                "{name}"
            );
        }
    }

    #[test]
    fn the_words_and_the_date_are_what_the_owner_asked_to_be_shown() {
        assert_eq!(
            ReauthReason::TestingExpiry.plain(),
            "Google signs this app out every 7 days until it is verified."
        );
        assert!(!ReauthReason::Revoked.plain().is_empty());
        assert_eq!(
            testing_expiry_seam(UnixSeconds(1_000)),
            UnixSeconds(1_000 + 7 * 86_400)
        );
    }
}
