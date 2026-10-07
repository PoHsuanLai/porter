//! What accountd answers.

use crate::candidate::Candidate;
use crate::consent::{Availability, Grant};
use crate::id::AccountId;
use crate::token::IssuedToken;
use serde::{Deserialize, Serialize};

/// One reply from accountd, matching its [`crate::AccountsRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum AccountsReply {
    /// For `Query`: the granted accounts that fit, best first.
    Candidates(Vec<Candidate>),
    /// For `Availability`.
    Availability(Availability),
    /// For `Choose`: the account picked, with its new grant.
    Chosen(Candidate),
    /// For `AddAccount`: the account added.
    Added(AccountId),
    /// For `Reauthenticate`: signed in again.
    Reauthenticated,
    /// For `ListGrants`.
    Grants(Vec<Grant>),
    /// For `Revoke`.
    Revoked,
    /// For `IssueToken`.
    Token(IssuedToken),
    /// For `OpenAuthenticated`: the relay is running and its descriptor travels beside this
    /// reply, out of band (a D-Bus `h`, an `SCM_RIGHTS` descriptor on the socket frame, an
    /// in-memory duplex in process). The reply itself carries no value.
    Authenticated,
    /// The request was refused, and why.
    Refused(Refusal),
}

/// Why accountd refused a request; each is something the app can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The user closed the sheet.
    Dismissed,
    /// The user refused (now or before).
    Denied,
    /// No account fits; offer "Add Account…".
    NoFittingAccount,
    /// The grant is not the caller's, or no longer exists.
    UnknownGrant,
    /// The grant does not cover that audience.
    AudienceNotGranted,
    /// The account must be signed in again (`Reauthenticate`).
    NeedsReauth,
    /// The account or the secret store cannot be reached.
    Unavailable,
    /// The endpoint is not one of the account's for the grant's kind: the relay dials only
    /// what the account holds, never an address the app names.
    EndpointNotGranted,
    /// An agent account asked to sign in or out, and no launcher has registered the agent's
    /// program: only the agent can log itself in, and nobody is there to ask it.
    NoLauncher,
}
