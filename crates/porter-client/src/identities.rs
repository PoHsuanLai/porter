//! The identities signed in on this machine (`org.quire.Identities1` on accountd), for an app
//! that wants to say "Continue as ada@example.org" without holding the sign-in itself. An
//! identity is an address, a provider and a kind; nothing here is a token.
//!
//! ```ignore
//! let identities = transport.identities().await?;
//! match identities.offer(&ProviderHint::Any, &window).await {   // the person picks in a sheet
//!     Ok(picked) => {
//!         // Sign in to your own service as `picked.address`, then say so:
//!         identities.register_use(&picked.id).await?;
//!         // A password identity may hand its password over, once, right after the pick:
//!         let password = identities.take_password(&picked.id).await?;
//!     }
//!     Err(IdentitiesError::Cancelled) => { /* the person closed the sheet */ }
//!     Err(IdentitiesError::NoIdentity) => { /* offer to sign one in */ }
//!     Err(other) => show(other.to_string()),
//! }
//! let mut removed = identities.watch_removals().await?;        // subscribe before reading the list
//! while let Some(id) = removed.next().await { /* forget your own copies of it */ }
//! ```

use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    AppName, CoreError, Identity, IdentityAddress, IdentityId, ProviderId, SecretText,
};
use porter_dbus::{
    BusConnection, BusError, BusFailure, BusStream, IdentitiesProxy, IdentityRemoved,
    IdentityRemovedStream, classify, identity_from_dbus, is_invalid_args, is_limits_exceeded,
    refusal_of,
};
use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Why an Identities call failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IdentitiesError {
    /// The person closed the sheet without picking an identity.
    #[error("the person did not pick an identity")]
    Cancelled,
    /// No identity fits: none is signed in on this machine, or none of the provider asked for.
    #[error("no identity is signed in")]
    NoIdentity,
    /// This app has an identity picker open already.
    #[error("an identity picker is open already")]
    Busy,
    /// accountd refused an argument (an identity that is not there, a provider that is not one).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// accountd does not know this caller, or the call is not one it may make, or the password
    /// was not picked for it just now.
    #[error("refused by accountd: {0}")]
    Denied(String),
    /// accountd refused the call for a reason it names.
    #[error("refused by accountd: {0:?}")]
    Refused(Refusal),
    /// The password of the identity is gone: it needs signing in again.
    #[error("the identity must be signed in again")]
    NeedsSignIn,
    /// accountd is there but cannot do it now (no secret store, no way to show the picker).
    #[error("the account service cannot do that now")]
    Unavailable,
    /// accountd is not there.
    #[error("no account service reachable")]
    Unreachable,
    /// accountd sent something that is not porter's protocol, or could not save.
    #[error("malformed: {0}")]
    Malformed(String),
}

impl From<BusError> for IdentitiesError {
    fn from(error: BusError) -> Self {
        if let Some(refusal) = refusal_of(&error) {
            return match refusal {
                Refusal::Dismissed => IdentitiesError::Cancelled,
                Refusal::NoFittingAccount => IdentitiesError::NoIdentity,
                Refusal::NeedsReauth => IdentitiesError::NeedsSignIn,
                Refusal::Unavailable => IdentitiesError::Unavailable,
                other => IdentitiesError::Refused(other),
            };
        }
        if is_limits_exceeded(&error) {
            return IdentitiesError::Busy;
        }
        if is_invalid_args(&error) {
            return IdentitiesError::Invalid(error.to_string());
        }
        match classify(&error) {
            BusFailure::NoDaemon => IdentitiesError::Unreachable,
            BusFailure::Denied(why) => IdentitiesError::Denied(why),
            BusFailure::Other(why) => IdentitiesError::Malformed(why),
            // a failure a later porter-dbus adds
            _ => IdentitiesError::Malformed("a bus failure this client does not know".into()),
        }
    }
}

impl From<CoreError> for IdentitiesError {
    fn from(error: CoreError) -> Self {
        IdentitiesError::Malformed(error.to_string())
    }
}

/// The identity the person picked in `Identities::offer`: who to continue as. It holds no token.
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

/// accountd's identities, as an app sees them.
#[derive(Debug, Clone)]
pub struct Identities {
    proxy: IdentitiesProxy<'static>,
}

impl Identities {
    /// The identities over `connection`.
    pub async fn connect(connection: &BusConnection) -> Result<Self, IdentitiesError> {
        Self::connect_at(connection, &crate::Destination::WellKnown).await
    }

    /// The same, sent to `to`: the owner of accountd's name the caller checked.
    pub async fn connect_at(
        connection: &BusConnection,
        to: &crate::Destination,
    ) -> Result<Self, IdentitiesError> {
        Ok(Self {
            proxy: to.proxy(connection).await?,
        })
    }

    /// The identities signed in on this machine, in accountd's order. Local runtimes, Tailscale,
    /// AI keys and agent logins are not among them. A row that cannot be read fails the list.
    pub async fn list(&self) -> Result<Vec<Identity>, IdentitiesError> {
        self.proxy
            .list()
            .await?
            .iter()
            .map(|row| Ok(identity_from_dbus(row)?))
            .collect()
    }

    /// "Continue as": shows the person the identity picker and waits for the pick. `hint` limits
    /// it to one provider's identities, or [`ProviderHint::Any`]; the sheet sits over `window`. The app has one picker open at a
    /// time ([`IdentitiesError::Busy`]); the person closing it is [`IdentitiesError::Cancelled`].
    pub async fn offer(
        &self,
        hint: &ProviderHint,
        window: &ParentWindow,
    ) -> Result<Picked, IdentitiesError> {
        let word = match hint {
            ProviderHint::Provider(provider) => provider.as_str(),
            _ => "",
        };
        let parent = match window {
            ParentWindow::Handle(handle) => handle.as_str(),
            _ => "",
        };
        let (id, address, provider) = self.proxy.offer(word, parent).await?;
        Ok(Picked {
            id: IdentityId::parse(&id)?,
            address: IdentityAddress(address),
            provider: ProviderId::parse(&provider)?,
        })
    }

    /// The password (or app password) of the identity the person just picked in this app's
    /// [`Identities::offer`]. It is handed over once per pick, only for a password identity, and
    /// accountd records that it was; any other call is [`IdentitiesError::Denied`]. Keep it in
    /// the app's own secret store.
    pub async fn take_password(&self, id: &IdentityId) -> Result<SecretText, IdentitiesError> {
        let fd = self.proxy.take_password(id.as_str()).await?;
        let mut text = String::new();
        std::fs::File::from(std::os::fd::OwnedFd::from(fd))
            .read_to_string(&mut text)
            .map_err(|why| IdentitiesError::Malformed(format!("the password: {why}")))?;
        Ok(SecretText::new(text))
    }

    /// Says that this app signed in with `id`: it joins the list Settings shows as the apps using
    /// the identity. Saying it again changes nothing.
    pub async fn register_use(&self, id: &IdentityId) -> Result<(), IdentitiesError> {
        Ok(self.proxy.register_use(id.as_str()).await?)
    }

    /// The apps that signed in with `id`. Only Settings and the shell may ask.
    pub async fn uses(&self, id: &IdentityId) -> Result<Vec<AppName>, IdentitiesError> {
        self.proxy
            .uses(id.as_str())
            .await?
            .iter()
            .map(|name| Ok(AppName::parse(name)?))
            .collect()
    }

    /// The identities removed from now on. It subscribes, then reads the list once so accountd
    /// counts this connection among those it tells.
    pub async fn watch_removals(&self) -> Result<IdentityRemovals, IdentitiesError> {
        let stream = self.proxy.receive_identity_removed().await?;
        self.proxy.list().await?;
        Ok(IdentityRemovals(stream))
    }
}

/// The stream of removals: the id of each identity Settings removed. An app decides what to do
/// with its own copies. An item is an error when accountd sent one that is not well formed.
#[derive(Debug)]
pub struct IdentityRemovals(IdentityRemovedStream);

fn removed(signal: IdentityRemoved) -> Result<IdentityId, IdentitiesError> {
    let args = signal.args()?;
    Ok(IdentityId::parse(args.identity_id)?)
}

impl BusStream for IdentityRemovals {
    type Item = Result<IdentityId, IdentitiesError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map(|next| next.map(removed))
    }
}
