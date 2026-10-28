//! What the app is told when a provider, the secret store or a sheet fails.

use porter_provider::ProviderError;
use porter_secrets::SecretsError;
use porter_sheet::SignInFault;
use porter_wire::Refusal;

/// What the app is told when the provider fails.
pub fn provider_refusal(error: ProviderError) -> Refusal {
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
pub fn secrets_refusal(error: SecretsError) -> Refusal {
    match error {
        SecretsError::Missing | SecretsError::Unreadable => Refusal::NeedsReauth,
        // locked, unreachable, and a failure a later porter-secrets adds
        _ => Refusal::Unavailable,
    }
}

/// What an app is told when the sheet ends without an account.
pub fn refusal_of(fault: SignInFault) -> Refusal {
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
}
