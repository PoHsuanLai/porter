//! `Accounts`: the app's handle on porter.

use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::error::ClientError;
#[cfg(feature = "dbus")]
use crate::error::TransportError;
use crate::found::ConsentOffer;
use crate::transport::Transport;
use porter_core::ProviderHint;
use porter_core::{AccountId, GrantId};
use porter_core::{Candidate, EndpointUrl};
use porter_wire::ParentWindow;
use porter_wire::{AccountsReply, AccountsRequest};

/// The app's connection to the account service.
#[derive(Debug)]
pub struct Accounts<T> {
    pub(crate) transport: T,
}

#[cfg(feature = "dbus")]
impl Accounts<crate::transport::DbusTransport> {
    /// Connects over the session bus. accountd is found, and started by bus activation, at the
    /// first call. No session bus is `TransportError::Unreachable`.
    pub async fn connect() -> Result<Self, ClientError> {
        crate::transport::DbusTransport::session()
            .await
            .map(|transport| Self { transport })
            .map_err(|_| TransportError::Unreachable.into())
    }

    /// The identities accountd removes, from now on (`Identities::watch_removals`, as accounts
    /// by their object-path segment). An app that keeps something of an identity (a copy of its
    /// sign-in, mirrors) forgets it on each [`crate::RemovedAccount`].
    pub async fn watch_removals(&self) -> Result<crate::Removals, ClientError> {
        Ok(crate::Removals::watch(self.transport.connection(), self.transport.accounts()).await?)
    }
}

impl<T: Transport> Accounts<T> {
    /// Over a transport the app built (`InProcess` for an app hosting the core).
    pub fn over(transport: T) -> Self {
        Self { transport }
    }

    /// Shows the chooser and consent sheet for `offer`; the account the user picked, granted.
    pub async fn request_grant(
        &self,
        offer: &ConsentOffer,
        window: &ParentWindow,
    ) -> Result<Candidate, ClientError> {
        let request = AccountsRequest::Choose {
            need: offer.need.clone(),
            class: offer.class,
            usage: offer.usage,
            window: window.clone(),
        };
        match self.transport.call(request).await? {
            AccountsReply::Chosen(candidate) => Ok(candidate),
            other => Err(unexpected(other)),
        }
    }

    /// Opens the add-account sheet; the account added. An account the person signs in to that
    /// was already here is `ClientError::AlreadyAdded` with its id: nothing was added, and the app
    /// may ask for a grant of it.
    pub async fn add_account(
        &self,
        hint: ProviderHint,
        window: &ParentWindow,
    ) -> Result<AccountId, ClientError> {
        let request = AccountsRequest::AddAccount {
            hint,
            window: window.clone(),
        };
        match self.transport.call(request).await? {
            AccountsReply::Added(account) => Ok(account),
            AccountsReply::AlreadyAdded(account) => Err(ClientError::AlreadyAdded(account)),
            other => Err(unexpected(other)),
        }
    }

    /// Opens the sheet that signs a granted account in again (its refresh token was revoked, a
    /// password changed). An account the app holds no grant for is not one it can see.
    pub async fn reauthenticate(
        &self,
        account: &AccountId,
        window: &ParentWindow,
    ) -> Result<(), ClientError> {
        let request = AccountsRequest::Reauthenticate {
            account: account.clone(),
            window: window.clone(),
        };
        match self.transport.call(request).await? {
            AccountsReply::Reauthenticated => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// A byte stream to `endpoint`, one of the account's servers the candidate listed, already
    /// authenticated by a daemon-side relay: an IMAP, SMTP or HTTP engine speaks its protocol on
    /// it and never holds the password. The IMAP stream starts with a `PREAUTH` greeting, the
    /// SMTP one with a `220` and an `EHLO` reply that offers no `AUTH` and no `STARTTLS`, and the
    /// HTTP one takes plain HTTP/1.1 requests (the relay adds `Authorization`, refuses any other
    /// origin and strips the app's own `Authorization`).
    pub async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<AuthenticatedStream, ClientError> {
        match self.transport.open_authenticated(grant, endpoint).await? {
            Relayed::Stream(stream) => Ok(stream),
            Relayed::Refused(refusal) => Err(ClientError::Refused(refusal)),
        }
    }

    /// A byte stream to `origin` (`https://host[:port]`, no path), a host the account's provider
    /// file declares as one its pre-authenticated links point at: Microsoft Graph's `uploadUrl`
    /// and the `downloadUrl` a content request redirects to. The relay speaks TLS and plain
    /// HTTP/1.1 on the stream and adds **no** credential (the link carries its own), drops any
    /// `Authorization` the app writes and refuses any other origin. An origin the file does not
    /// declare is `Refused(EndpointNotGranted)`.
    pub async fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<AuthenticatedStream, ClientError> {
        match self.transport.open_linked(grant, origin).await? {
            Relayed::Stream(stream) => Ok(stream),
            Relayed::Refused(refusal) => Err(ClientError::Refused(refusal)),
        }
    }
}

/// A refusal as its error; any other reply answered a different request.
fn unexpected(reply: AccountsReply) -> ClientError {
    match reply {
        AccountsReply::Refused(refusal) => ClientError::Refused(refusal),
        _ => ClientError::Mismatched,
    }
}
