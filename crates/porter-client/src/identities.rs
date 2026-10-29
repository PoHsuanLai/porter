//! The identities signed in on this machine (`org.quire.Identities1` on accountd), for an app
//! that wants to say "Continue as ada@example.org" without holding the sign-in itself. An
//! identity is an address, a provider and a kind; nothing here is a token.
//!
//! ```ignore
//! let identities = porter.identities();
//! match identities.offer(&ProviderHint::Any, &window).await {   // the person picks in a sheet
//!     Ok(picked) => {
//!         // Sign in to your own service as `picked.address`, then say so:
//!         identities.register_use(&picked.id).await?;
//!         // A password identity may hand its password over, once, right after the pick:
//!         let password = identities.take_password(&picked.id).await?;
//!     }
//!     Err(IdentitiesError::Cancelled) => { /* the person closed the sheet */ }
//!     Err(IdentitiesError::NoIdentity) => { /* offer to sign one in */ }
//!     Err(other) => report(other),
//! }
//! let mut removed = identities.watch_removals().await?;        // subscribe before reading the list
//! while let Some(id) = removed.next().await { /* forget your own copies of it */ }
//! ```

use crate::changes::Changes;
use crate::error::LinkError;
use crate::link::IdentitiesLink;
use porter_core::ProviderHint;
use porter_core::ProviderId;
use porter_core::{AppName, Identity, IdentityAddress, IdentityId, SecretText};
use porter_wire::{ParentWindow, Refusal};
use std::sync::Arc;

/// Why an Identities call failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IdentitiesError {
    /// The person closed the sheet without picking an identity.
    #[error("cancelled")]
    Cancelled,
    /// No identity fits: none is signed in on this machine, or none of the provider asked for.
    #[error("no identity")]
    NoIdentity,
    /// This app has an identity picker open already.
    #[error("an identity picker is open")]
    Busy,
    /// accountd refused the call for a reason it names.
    #[error("refused")]
    Refused(Refusal),
    /// The password of the identity is gone: it needs signing in again.
    #[error("needs sign-in")]
    NeedsSignIn,
    /// accountd is there but cannot do it now (no secret store, no way to show the picker).
    #[error("unavailable")]
    Unavailable,
    /// The call did not complete.
    #[error(transparent)]
    Link(#[from] LinkError),
}

/// The identity the person picked in [`Identities::offer`]: who to continue as. It holds no token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Picked {
    /// The identity's id, which `register_use` and `take_password` take.
    pub id: IdentityId,
    /// The address to continue as.
    pub address: IdentityAddress,
    /// The provider's id.
    pub provider: ProviderId,
}

impl Picked {
    /// The identity `id`, at `address`, of `provider`.
    pub fn new(id: IdentityId, address: IdentityAddress, provider: ProviderId) -> Self {
        Self {
            id,
            address,
            provider,
        }
    }
}

/// accountd's identities, as an app sees them.
#[derive(Debug)]
pub struct Identities<T> {
    pub(crate) link: Arc<T>,
}

impl<T> Clone for Identities<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: IdentitiesLink> Identities<T> {
    /// The identities signed in on this machine, in accountd's order. Local runtimes, Tailscale,
    /// AI keys and agent logins are not among them. A row that cannot be read fails the list.
    pub async fn list(&self) -> Result<Vec<Identity>, IdentitiesError> {
        self.link.list().await
    }

    /// "Continue as": shows the person the identity picker and waits for the pick. `hint` limits
    /// it to one provider's identities, or [`ProviderHint::Any`]; the sheet sits over `window`.
    /// The app has one picker open at a time ([`IdentitiesError::Busy`]); the person closing it is
    /// [`IdentitiesError::Cancelled`].
    pub async fn offer(
        &self,
        hint: &ProviderHint,
        window: &ParentWindow,
    ) -> Result<Picked, IdentitiesError> {
        self.link.offer(hint, window).await
    }

    /// The password (or app password) of the identity the person just picked in this app's
    /// [`Identities::offer`]. It is handed over once per pick, only for a password identity, and
    /// accountd records that it was; any other call is `Link(LinkError::Denied)`. Keep it in the
    /// app's own secret store.
    pub async fn take_password(&self, id: &IdentityId) -> Result<SecretText, IdentitiesError> {
        self.link.take_password(id).await
    }

    /// Says that this app signed in with `id`: it joins the list Settings shows as the apps using
    /// the identity. Saying it again changes nothing.
    pub async fn register_use(&self, id: &IdentityId) -> Result<(), IdentitiesError> {
        self.link.register_use(id).await
    }

    /// The apps that signed in with `id`. Only Settings and the shell may ask.
    pub async fn uses(&self, id: &IdentityId) -> Result<Vec<AppName>, IdentitiesError> {
        self.link.uses(id).await
    }

    /// The identities removed from now on. It subscribes, then reads the list once so accountd
    /// counts this connection among those it tells.
    pub async fn watch_removals(&self) -> Result<Changes<IdentityId>, IdentitiesError> {
        self.link.watch_removals().await
    }
}
