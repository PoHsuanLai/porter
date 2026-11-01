//! Why an edit of an account's servers was refused.

use porter_sheet::ServersFixed;
use porter_wire::Refusal;
use std::fmt;

/// Why [`AccountsApi::edit_servers`](crate::AccountsApi::edit_servers) gave nothing back. More
/// reasons may be added: match with a wildcard. Nothing here is a sentence for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditError {
    /// The provider's sign-in is not a typed password, so its servers are not the person's to
    /// name.
    Fixed(ServersFixed),
    /// The service holds the account under another provider than the edit names.
    NotThatProvider,
    /// The sheet ended without servers: the person closed it, or the host could not draw it.
    Refused(Refusal),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EditError::Fixed(_) => "fixed",
            EditError::NotThatProvider => "not that provider",
            EditError::Refused(_) => "refused",
        })
    }
}

impl std::error::Error for EditError {}
