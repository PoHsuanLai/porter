//! The seam each protocol family implements, and the fake porter-fake drives (CONVENTIONS §5).

use crate::error::ProviderError;
use crate::spec::ProviderSpec;
use porter_core::{AccountId, Audience, AuthKind, Claim, Credential, IssuedToken};
use std::future::Future;

/// What an account presents when it talks to its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presented {
    /// Nothing: a local runtime, or an endpoint that needs no credential.
    Anonymous,
    /// The account's stored credential.
    Credential(Credential),
}

/// A provider as code: one family (or a fake) serving the accounts made from one
/// [`ProviderSpec`].
pub trait Provider: Send + Sync {
    /// The live connection an open account holds.
    type Session: ProviderSession;

    /// The declaration this provider serves.
    fn spec(&self) -> &ProviderSpec;

    /// How its accounts sign in.
    fn auth_kind(&self) -> AuthKind {
        self.spec().auth.kind
    }

    /// What the account can do, as the protocol's own session answer says (at add time and on
    /// every reconnect): claims at `Discovered` or `Probed` provenance.
    fn discover(
        &self,
        account: &AccountId,
        presented: &Presented,
    ) -> impl Future<Output = Result<Vec<Claim>, ProviderError>> + Send;

    /// Opens the account: the session mints the short-lived tokens apps receive.
    fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> impl Future<Output = Result<Self::Session, ProviderError>> + Send;
}

/// An open account: the only holder of its long-lived credential, inside accountd.
pub trait ProviderSession: Send + Sync {
    /// A short-lived token for `audience`, renewed from the refresh token when due.
    fn access_token(
        &self,
        audience: &Audience,
    ) -> impl Future<Output = Result<IssuedToken, ProviderError>> + Send;

    /// The credential to store again when renewal changed it (a rotated refresh token), or
    /// `None` when it is unchanged.
    fn renewed(&self) -> Option<Credential>;
}
