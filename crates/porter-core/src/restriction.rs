//! Why an account is limited, for the one secondary line Settings shows (design/31 §2.5).

use crate::capability::CapabilityKind;
use crate::units::Count;
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
}

impl Restriction {
    /// An account nothing limits.
    pub fn none() -> Self {
        Self {
            verification: Verification::NotNeeded,
            token_lifetime: TokenLifetime::Standard,
            consent: TenantConsent::User,
            limits: Vec::new(),
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
