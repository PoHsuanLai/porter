//! Why an account is limited, for the one secondary line Settings shows (design/31 §2.5).

use crate::capability::CapabilityKind;
use crate::units::{Count, UnixSeconds};
use serde::{Deserialize, Serialize};

/// Everything that limits an account beyond its capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Restriction {
    /// The OAuth client's verification state for this account's scopes.
    pub verification: Verification,
    /// How long a sign-in lasts.
    pub token_lifetime: TokenLifetime,
    /// Whether the organisation must consent.
    pub consent: TenantConsent,
    /// Per-kind limits (Google: Photos picker only, Drive app folder only).
    pub limits: Vec<Limit>,
    /// When the person last signed in, set by the family for a sign-in that expires
    /// (`TokenLifetime::SevenDays`); `None` where it does not matter or is not known (a
    /// registry written before vocabulary 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in: Option<UnixSeconds>,
}

/// Seconds a sign-in through a client in testing lasts (Google: seven days).
pub const SEVEN_DAYS: i64 = 7 * 24 * 60 * 60;

/// A refusal this young cannot be the seven days: the person ended it, or the password changed.
/// A day short of the seven is allowed for clocks and for how the provider counts.
const SEVEN_DAY_REFUSAL_FROM: i64 = 6 * 24 * 60 * 60;

impl Restriction {
    /// An account nothing limits.
    pub fn none() -> Self {
        Self {
            verification: Verification::NotNeeded,
            token_lifetime: TokenLifetime::Standard,
            consent: TenantConsent::User,
            limits: Vec::new(),
            signed_in: None,
        }
    }

    /// When a sign-in that lasts seven days stops working: what Settings shows. `None` for any
    /// other lifetime or when the sign-in's date is not known.
    pub fn expires_at(&self) -> Option<UnixSeconds> {
        match (self.token_lifetime, self.signed_in) {
            (TokenLifetime::SevenDays, Some(at)) => Some(UnixSeconds(at.0 + SEVEN_DAYS)),
            _ => None,
        }
    }

    /// Why an account the provider refused (`NeedsReauth`) was refused, when it is the seven
    /// days: the lifetime is seven days and the sign-in is old enough, or of unknown age.
    pub fn reauth_reason(&self, now: UnixSeconds) -> Option<ReauthReason> {
        let aged = self
            .signed_in
            .is_none_or(|at| now.0 - at.0 >= SEVEN_DAY_REFUSAL_FROM);
        match (self.token_lifetime, aged) {
            (TokenLifetime::SevenDays, true) => Some(ReauthReason::TestingAppExpired),
            _ => None,
        }
    }
}

/// Why an account has to be signed in again, when porter knows beyond "the provider refused".
/// Closed: the sheet words each, nothing from the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReauthReason {
    /// The provider signs a client that is still in testing out every seven days (R5).
    TestingAppExpired,
}

impl ReauthReason {
    /// The word on the bus and in the audit trail.
    pub fn slug(self) -> &'static str {
        match self {
            ReauthReason::TestingAppExpired => "testing_app_expired",
        }
    }

    /// The reason a word stands for.
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "testing_app_expired" => Some(ReauthReason::TestingAppExpired),
            _ => None,
        }
    }
}

/// The OAuth client's verification for the scopes an account uses (C2, C3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Verification {
    /// The scopes need none, or the auth is not OAuth.
    NotNeeded,
    /// The client is verified.
    Verified,
    /// Unverified: a warning screen and at most this many users.
    Unverified {
        /// The provider's user cap.
        user_cap: Count,
    },
}

/// How long a sign-in lasts before the user must sign in again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenLifetime {
    /// Until revoked.
    Standard,
    /// Seven days (a Google client in Testing mode, R5): Settings shows a reminder.
    SevenDays,
    /// Until the provider password changes (iCloud app passwords, C4).
    UntilPasswordChange,
}

/// Whether an organisation's admin must consent (C11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantConsent {
    /// The user's own consent is enough.
    User,
    /// An admin must consent and has not.
    AdminRequired,
    /// An admin has consented.
    AdminGranted,
}

/// One kind's limit and its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Limit {
    /// The kind limited.
    pub kind: CapabilityKind,
    /// Why.
    pub reason: LimitReason,
}

/// Why a present kind is less than the provider's full product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitReason {
    /// Uploads only; existing items are not readable.
    AppendOnly,
    /// Existing items only through a provider-drawn picker.
    PickerOnly,
    /// Only files this desktop created.
    AppFolderOnly,
    /// The provider offers no access.
    ProviderOffersNone,
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn testing(signed_in: Option<i64>) -> Restriction {
        Restriction {
            token_lifetime: TokenLifetime::SevenDays,
            signed_in: signed_in.map(UnixSeconds),
            ..Restriction::none()
        }
    }

    #[test]
    fn the_expiry_is_seven_days_after_a_known_sign_in_and_only_then() {
        let cases = [
            (
                "testing, known",
                testing(Some(1_000)),
                Some(1_000 + 7 * DAY),
            ),
            ("testing, unknown", testing(None), None),
            (
                "standard",
                Restriction {
                    signed_in: Some(UnixSeconds(5)),
                    ..Restriction::none()
                },
                None,
            ),
        ];
        for (name, restriction, want) in cases {
            assert_eq!(restriction.expires_at(), want.map(UnixSeconds), "{name}");
        }
    }

    #[test]
    fn a_refusal_is_the_seven_days_only_for_that_lifetime_and_an_old_enough_sign_in() {
        let now = UnixSeconds(100 * DAY);
        let ago = |days: i64| Some(100 * DAY - days * DAY);
        let cases = [
            ("a week old", testing(ago(7)), true),
            ("six days", testing(ago(6)), true),
            ("a day old", testing(ago(1)), false),
            ("age unknown", testing(None), true),
            ("standard, old", Restriction::none(), false),
            (
                "standard, with a date",
                Restriction {
                    signed_in: Some(UnixSeconds(0)),
                    ..Restriction::none()
                },
                false,
            ),
        ];
        for (name, restriction, seven) in cases {
            let want = seven.then_some(ReauthReason::TestingAppExpired);
            assert_eq!(restriction.reauth_reason(now), want, "{name}");
        }
    }

    #[test]
    fn the_reason_has_one_word_that_reads_back_and_nothing_else_does() {
        let word = ReauthReason::TestingAppExpired.slug();
        assert_eq!(word, "testing_app_expired");
        assert_eq!(
            ReauthReason::from_slug(word),
            Some(ReauthReason::TestingAppExpired)
        );
        assert_eq!(ReauthReason::from_slug("Testing app expired."), None);
        assert_eq!(
            serde_json::to_value(ReauthReason::TestingAppExpired).unwrap(),
            word
        );
    }

    #[test]
    fn a_restriction_written_before_the_sign_in_date_still_reads() {
        let old = r#"{"verification":{"kind":"unverified","v":{"user_cap":100}},
            "token_lifetime":"seven_days","consent":"user","limits":[]}"#;
        let read: Restriction = serde_json::from_str(old).expect("reads");
        assert_eq!(read.token_lifetime, TokenLifetime::SevenDays);
        assert_eq!(read.signed_in, None);
        let written = serde_json::to_string(&read).expect("json");
        assert!(!written.contains("signed_in"));
    }
}
