//! Why Google refused a sign-in, and the seven day rule behind the one that is Google's doing.
//! The reason and its word are core's (`porter_core::ReauthReason`).
//!
//! An OAuth client whose consent screen is in *testing* (the state every client the owner
//! registers starts in, and stays in until Google verifies it) gets refresh tokens that Google
//! ends after seven days (design/31 R5). The refusal looks like any other `invalid_grant`, so the
//! family says which it is from the client's row (`testing = true`) and, where it knows when the
//! person signed in, from the age of the sign-in.

use porter_core::ReauthReason;
use porter_ids::UnixSeconds;

/// Why Google refused a renewal, when it is the seven days: the client is in testing and the
/// sign-in is old enough, or of unknown age (core's one rule, `ReauthReason::of_seven_days`). A
/// young sign-in was ended by someone, and any other refusal has no reason porter knows.
pub(super) fn reason_of_refusal(
    testing: bool,
    signed_in: Option<UnixSeconds>,
    now: UnixSeconds,
) -> Option<ReauthReason> {
    testing
        .then(|| ReauthReason::of_seven_days(signed_in, now))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_the_seven_day_rule_only_for_a_client_in_testing_and_old_enough() {
        const DAY: i64 = 86_400;
        let now = UnixSeconds(100 * DAY);
        let at = |days_ago: i64| Some(UnixSeconds(now.0 - days_ago * DAY));
        const T: bool = true;
        const V: bool = false;
        let seven = Some(ReauthReason::TestingAppExpired);
        let cases = [
            ("testing, a week old", T, at(7), seven),
            ("testing, six and a half", T, at(6), seven),
            ("testing, a day old", T, at(1), None),
            ("testing, age unknown", T, None, seven),
            ("verified, a week old", V, at(7), None),
            ("verified, age unknown", V, None, None),
        ];
        for (name, testing, signed_in, want) in cases {
            assert_eq!(reason_of_refusal(testing, signed_in, now), want, "{name}");
        }
    }
}
