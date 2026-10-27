//! `IssueToken`: which secret an account presents, and minting a short-lived token from it.

use porter_core::sheet::SignInFault;
use porter_core::wire::Refusal;
use porter_core::{AuthKind, SecretPurpose};
use porter_provider::ProviderError;
use porter_secrets::SecretsError;

/// The secret an account of `auth` presents to its provider, or `None` for kinds with none.
pub fn secret_purpose(auth: AuthKind) -> Option<SecretPurpose> {
    match auth {
        // An agent that signs itself in holds its login itself, and so does Tailscale; porter
        // files nothing.
        AuthKind::None | AuthKind::LocalRuntime | AuthKind::AgentLogin | AuthKind::OwnProgram => {
            None
        }
        AuthKind::Password
        | AuthKind::AppPassword
        | AuthKind::LoginFlowV2
        | AuthKind::LocalBridge => Some(SecretPurpose::Password),
        AuthKind::KeyPair => Some(SecretPurpose::KeyPair),
        AuthKind::ApiKey | AuthKind::OAuthMintsKey => Some(SecretPurpose::ApiKey),
        AuthKind::OAuthPkce | AuthKind::OAuthPlan => Some(SecretPurpose::OAuthRefresh),
        AuthKind::CloudIdentity => Some(SecretPurpose::OAuthRefresh),
    }
}

/// What the app is told when the provider fails.
pub(crate) fn provider_refusal(error: ProviderError) -> Refusal {
    match error {
        ProviderError::Unauthorized => Refusal::NeedsReauth,
        ProviderError::Forbidden => Refusal::Denied,
        ProviderError::Unreachable | ProviderError::Unreadable => Refusal::Unavailable,
        // The account is fine; it was never given this access. Not a fault to wait out.
        ProviderError::NeedsConsent => Refusal::NeedsConsent,
        // a variant a newer porter adds: the app is told it is unavailable
        _ => Refusal::Unavailable,
    }
}

/// What the app is told when the secret store fails: a missing secret means signing in again.
pub(crate) fn secrets_refusal(error: SecretsError) -> Refusal {
    match error {
        SecretsError::Missing | SecretsError::Unreadable => Refusal::NeedsReauth,
        // locked, unreachable, and a failure a later porter-secrets adds
        _ => Refusal::Unavailable,
    }
}

/// What an app is told when the sheet ends without an account.
pub(crate) fn refusal_of(fault: SignInFault) -> Refusal {
    match fault {
        SignInFault::Cancelled => Refusal::Dismissed,
        SignInFault::NoLauncher => Refusal::NoLauncher,
        SignInFault::Forbidden | SignInFault::Refused => Refusal::Denied,
        SignInFault::Unreachable
        | SignInFault::Unreadable
        | SignInFault::NeedsClientId
        | SignInFault::TimedOut
        | SignInFault::Expired
        | SignInFault::NotInstalled
        | SignInFault::NotRunning
        | SignInFault::SignedOut
        | SignInFault::NotAllowed
        | SignInFault::AlreadyAdded
        | SignInFault::StoreFailed => Refusal::Unavailable,
        // a variant a newer porter adds: the app is told it is unavailable
        _ => Refusal::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_fault_is_told_to_the_app_as_what_it_can_act_on() {
        let cases = [
            (ProviderError::Unauthorized, Refusal::NeedsReauth),
            (ProviderError::Forbidden, Refusal::Denied),
            (ProviderError::Unreachable, Refusal::Unavailable),
            (ProviderError::Unreadable, Refusal::Unavailable),
            (ProviderError::NeedsConsent, Refusal::NeedsConsent),
        ];
        for (error, want) in cases {
            assert_eq!(provider_refusal(error), want, "{error:?}");
        }
    }

    #[test]
    fn each_auth_kind_presents_its_secret() {
        let cases = [
            (AuthKind::LocalRuntime, None),
            (AuthKind::AgentLogin, None),
            (AuthKind::OwnProgram, None),
            (AuthKind::None, None),
            (AuthKind::AppPassword, Some(SecretPurpose::Password)),
            (AuthKind::LoginFlowV2, Some(SecretPurpose::Password)),
            (AuthKind::KeyPair, Some(SecretPurpose::KeyPair)),
            (AuthKind::OAuthMintsKey, Some(SecretPurpose::ApiKey)),
            (AuthKind::OAuthPkce, Some(SecretPurpose::OAuthRefresh)),
        ];
        for (auth, expected) in cases {
            assert_eq!(secret_purpose(auth), expected, "{auth:?}");
        }
    }
}
