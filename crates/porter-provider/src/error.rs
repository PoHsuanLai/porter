//! porter-provider's errors: a refused provider file, and a provider call that failed.

use porter_core::ProviderId;

/// Why a provider file was refused. accountd logs it and skips the file; the rest load.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderFileError {
    /// Not valid TOML, or not the provider schema.
    #[error("provider file: {0}")]
    Syntax(String),
    /// An OAuth kind without an issuer, or an issuer on a kind that has none.
    #[error("provider {0}: auth issuer does not match its kind")]
    IssuerMismatch(ProviderId),
    /// No capability rows.
    #[error("provider {0}: declares no capability")]
    NoCapabilities(ProviderId),
    /// AI capability rows without an `[ai]` table, or an `[ai]` table without them.
    #[error("provider {0}: [ai] does not match its capabilities")]
    AiSpecMismatch(ProviderId),
    /// A language-model row whose `wire` is a different request format from its `family`'s.
    #[error("provider {0}: a language-model row's wire is not its family's")]
    WireFamily(ProviderId),
    /// Agent rows that break the rules: they are served by the `acp_agent` family and name each
    /// program once, and an `agent_login` provider declares nothing else.
    #[error("provider {0}: agent rows are not well formed")]
    AgentRows(ProviderId),
}

/// Why a call to a provider failed, in terms the account's state can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The credential was refused: the account needs signing in again.
    #[error("credential refused")]
    Unauthorized,
    /// The account's organisation or the provider forbids this (a tenant consent 403).
    #[error("forbidden")]
    Forbidden,
    /// The server could not be reached.
    #[error("unreachable")]
    Unreachable,
    /// The server answered something this family cannot read.
    #[error("unreadable answer")]
    Unreadable,
    /// The account was not given this access (it is asked when first used, or the person left
    /// it unticked): the credential is good, and only a sign-in that asks for the access
    /// (`SignInMode::Extend`) changes the answer.
    #[error("access not given")]
    NeedsConsent,
}

/// What a failed call says to the person signing in: the one mapping every family uses.
impl From<ProviderError> for porter_sheet::SignInFault {
    fn from(error: ProviderError) -> Self {
        use porter_sheet::SignInFault;
        match error {
            ProviderError::Unauthorized => SignInFault::Refused,
            ProviderError::Forbidden => SignInFault::Forbidden,
            ProviderError::Unreachable => SignInFault::Unreachable,
            ProviderError::Unreadable => SignInFault::Unreadable,
            // a variant a newer porter adds: the sign-in ends as unreadable
            _ => SignInFault::Unreadable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderError;
    use porter_sheet::SignInFault;

    #[test]
    fn a_failed_call_is_a_fault_the_person_can_read() {
        let cases = [
            (ProviderError::Unauthorized, SignInFault::Refused),
            (ProviderError::Forbidden, SignInFault::Forbidden),
            (ProviderError::Unreachable, SignInFault::Unreachable),
            (ProviderError::Unreadable, SignInFault::Unreadable),
            (ProviderError::NeedsConsent, SignInFault::Unreadable),
        ];
        for (error, want) in cases {
            assert_eq!(SignInFault::from(error), want, "{error:?}");
        }
    }
}
