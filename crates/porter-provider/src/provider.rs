//! The seam each protocol family implements, and the fake porter-fake drives (CONVENTIONS §5).

use crate::error::ProviderError;
use crate::sign_in::{RevokeOutcome, SignIn, SignInStart};
use crate::spec::ProviderSpec;
use porter_core::{
    Account, AccountId, Audience, AuthKind, CapabilityKind, Claim, Credential, IssuedToken,
};
use std::future::Future;

/// What an account presents when it talks to its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presented {
    /// Nothing: a local runtime, or an endpoint that needs no credential.
    Anonymous,
    /// The account's stored credential.
    Credential(Credential),
}

/// Whether a new account of a provider can be signed in now, which decides whether the add list
/// shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// It can.
    Ready,
    /// The issuer has no client for this build (no shipped row, none set in Settings): a sign-in
    /// could only say so.
    NeedsClient,
}

/// A provider as code: one family (or a fake) serving the accounts made from one
/// [`ProviderSpec`].
pub trait Provider: Send + Sync {
    /// The live connection an open account holds.
    type Session: ProviderSession;

    /// The conversation that signs an account in.
    type SignIn: SignIn;

    /// The declaration this provider serves.
    fn spec(&self) -> &ProviderSpec;

    /// How its accounts sign in.
    fn auth_kind(&self) -> AuthKind {
        self.spec().auth.kind
    }

    /// What the account can do, as the protocol's own session answer says (at add time and on
    /// every reconnect): claims at `Discovered` or `Probed` provenance. The account is the
    /// stored one (its endpoints and login name), so the family asks the servers it already knows.
    fn discover(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> impl Future<Output = Result<Vec<Claim>, ProviderError>> + Send;

    /// Opens the account: the session mints the short-lived tokens apps receive.
    fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> impl Future<Output = Result<Self::Session, ProviderError>> + Send;

    /// Begins signing an account in (a new one, or an existing one again). The host drives the
    /// returned conversation; nothing is stored until it ends in `Done`.
    fn sign_in(&self, start: SignInStart) -> Result<Self::SignIn, ProviderError>;

    /// Whether a new account can be signed in now. Asked each time the add list is drawn, so a
    /// client set while the host runs shows the provider at once. The default is `Ready`; the
    /// OAuth families answer `NeedsClient` while their issuer has no client.
    fn readiness(&self) -> Readiness {
        Readiness::Ready
    }

    /// Asks the provider to stop honouring `presented` (Google's revoke endpoint, Nextcloud's
    /// app-password delete), best effort, when an account is removed. The account names the
    /// servers (`Account::endpoints`) and the login a revoke is addressed to.
    fn revoke(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> impl Future<Output = Result<RevokeOutcome, ProviderError>> + Send;
}

/// An open account: the only holder of its long-lived credential, inside accountd.
pub trait ProviderSession: Send + Sync {
    /// A short-lived token for `audience`, renewed from the refresh token when due.
    fn access_token(
        &self,
        audience: &Audience,
    ) -> impl Future<Output = Result<IssuedToken, ProviderError>> + Send;

    /// A short-lived token for `audience` under a grant for `kind`: it reaches that kind's
    /// service and no other of the account's (a calendar grant's Microsoft token carries the
    /// calendar scope, not every Graph permission). The host asks this for every token an app
    /// receives or a relay presents. A family whose tokens are not per kind keeps the default,
    /// which is [`ProviderSession::access_token`].
    fn access_token_for(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
    ) -> impl Future<Output = Result<IssuedToken, ProviderError>> + Send {
        let _ = kind;
        self.access_token(audience)
    }

    /// The credential to store again when renewal changed it (a rotated refresh token), or
    /// `None` when it is unchanged.
    fn renewed(&self) -> Option<Credential>;
}
