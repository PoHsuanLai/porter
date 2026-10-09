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
    /// Agent rows that break the rules: they are served by the `acp_agent` family and name each
    /// program once, and an `agent_login` provider declares nothing else.
    #[error("provider {0}: agent rows are not well formed")]
    AgentRows(ProviderId),
}

/// Why a call to a provider failed, in terms the account's state can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
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
}
