//! `Grants`: who may use which account, and the relays a grant opens.

use crate::authenticated::AuthenticatedStream;
use crate::error::LinkError;
use crate::found::{ConsentOffer, Found, found};
use crate::link::GrantsLink;
use porter_core::consent::{Grant, Usage};
use porter_core::{Candidate, DataClass, EndpointUrl, GrantId};
use porter_match::Need;
use porter_wire::{ParentWindow, Refusal};
use std::sync::Arc;

/// Why a grants call failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GrantsError {
    /// accountd refused, or the person closed the sheet (`Refusal::Dismissed`).
    #[error("refused")]
    Refused(Refusal),
    /// The call did not complete.
    #[error(transparent)]
    Link(#[from] LinkError),
}

/// The grants domain over a transport.
#[derive(Debug)]
pub struct Grants<T> {
    pub(crate) link: Arc<T>,
}

impl<T> Clone for Grants<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: GrantsLink> Grants<T> {
    /// Whether an account can meet `need` for this app, and which.
    pub async fn find(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Found, GrantsError> {
        let candidates = self.link.candidates(need, class, usage).await?;
        let offer = ConsentOffer {
            need: need.clone(),
            class,
            usage,
        };
        if !candidates.is_empty() {
            return Ok(found(
                candidates,
                porter_core::consent::Availability::Granted,
                offer,
            ));
        }
        let availability = self.link.availability(need, class, usage).await?;
        Ok(found(candidates, availability, offer))
    }

    /// Shows the chooser and consent sheet for `offer`; the account the user picked, granted.
    pub async fn request_grant(
        &self,
        offer: &ConsentOffer,
        window: &ParentWindow,
    ) -> Result<Candidate, GrantsError> {
        self.link.request_grant(offer, window).await
    }

    /// The grants this app holds.
    pub async fn grants(&self) -> Result<Vec<Grant>, GrantsError> {
        self.link.grants().await
    }

    /// Withdraws one of this app's own grants.
    pub async fn revoke(&self, grant: &GrantId) -> Result<(), GrantsError> {
        self.link.revoke(grant).await
    }

    /// A byte stream to `endpoint`, one of the account's servers the candidate listed, already
    /// authenticated by a relay: an IMAP, SMTP or HTTP engine speaks its protocol on it and never
    /// holds the password. The IMAP stream starts with a `PREAUTH` greeting, the SMTP one with a
    /// `220` and an `EHLO` reply that offers no `AUTH` and no `STARTTLS`, and the HTTP one takes
    /// plain HTTP/1.1 requests (the relay adds `Authorization`, refuses any other origin and
    /// strips the app's own `Authorization`).
    pub async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<AuthenticatedStream, GrantsError> {
        self.link.open_authenticated(grant, endpoint).await
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
    ) -> Result<AuthenticatedStream, GrantsError> {
        self.link.open_linked(grant, origin).await
    }
}
