//! `Accounts`: the app's handle on porter.

use crate::env::{ClientEnv, LinkChoice};
use crate::error::{ClientError, TransportError};
use crate::found::{ConsentOffer, Found, found};
use crate::transport::{AnyTransport, Transport};
use porter_core::consent::{Grant, Usage};
use porter_core::wire::{ParentWindow, ProviderHint};
use porter_core::{
    AccountId, AccountsReply, AccountsRequest, Audience, Candidate, DataClass, GrantId,
    IssuedToken, Need, Tier,
};
use porter_infer::{ClientFrame, InferEvent, InferReply, InferRequest, InferSession, OpenOptions};

/// The app's connection to the account service.
#[derive(Debug)]
pub struct Accounts<T> {
    transport: T,
}

impl Accounts<AnyTransport> {
    /// Connects over the first reachable link in `env`, in order (D-Bus on the desktop, the
    /// latchkey socket elsewhere). A D-Bus link is reachable when the session bus is: the
    /// daemons are found, and started by bus activation, at the first call, so a machine with
    /// inferd and no accountd still connects. Without the `dbus` feature, and for a socket
    /// link (its carrier is not built), a link is skipped; no reachable link is
    /// `TransportError::Unreachable`.
    pub async fn connect(env: &ClientEnv) -> Result<Self, ClientError> {
        for link in &env.links {
            if let Some(transport) = reach(link).await {
                return Ok(Self { transport });
            }
        }
        Err(TransportError::Unreachable.into())
    }
}

/// The transport for one link, if it is reachable.
async fn reach(link: &LinkChoice) -> Option<AnyTransport> {
    match link {
        #[cfg(feature = "dbus")]
        LinkChoice::Dbus => crate::transport::DbusTransport::session()
            .await
            .ok()
            .map(AnyTransport::Dbus),
        #[cfg(not(feature = "dbus"))]
        LinkChoice::Dbus => None,
        LinkChoice::Socket(_) => None,
    }
}

impl<T: Transport> Accounts<T> {
    /// Over a transport the app built (`InProcess` for an app hosting the core).
    pub fn over(transport: T) -> Self {
        Self { transport }
    }

    /// Whether an account can meet `need` for this app, and which.
    pub async fn find(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Found, ClientError> {
        let query = AccountsRequest::Query {
            need: need.clone(),
            class,
            usage,
        };
        let candidates = match self.transport.call(query).await? {
            AccountsReply::Candidates(candidates) => candidates,
            other => return Err(unexpected(other)),
        };
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
        let ask = AccountsRequest::Availability {
            need: need.clone(),
            class,
            usage,
        };
        match self.transport.call(ask).await? {
            AccountsReply::Availability(availability) => Ok(found(candidates, availability, offer)),
            other => Err(unexpected(other)),
        }
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

    /// Opens the add-account sheet; the account added.
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
            other => Err(unexpected(other)),
        }
    }

    /// A short-lived token for a granted candidate; ask again when it expires or on a 401.
    pub async fn token(
        &self,
        candidate: &Candidate,
        audience: &Audience,
    ) -> Result<IssuedToken, ClientError> {
        let request = AccountsRequest::IssueToken {
            grant: candidate.grant.clone(),
            audience: audience.clone(),
        };
        match self.transport.call(request).await? {
            AccountsReply::Token(token) => Ok(token),
            other => Err(unexpected(other)),
        }
    }

    /// The app's own grants.
    pub async fn grants(&self) -> Result<Vec<Grant>, ClientError> {
        match self.transport.call(AccountsRequest::ListGrants).await? {
            AccountsReply::Grants(grants) => Ok(grants),
            other => Err(unexpected(other)),
        }
    }

    /// Withdraws one of the app's grants.
    pub async fn revoke(&self, grant: &GrantId) -> Result<(), ClientError> {
        match self
            .transport
            .call(AccountsRequest::Revoke {
                grant: grant.clone(),
            })
            .await?
        {
            AccountsReply::Revoked => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Opens a streaming session with inferd: write `ClientFrame`s, read `InferEvent`s. The
    /// route is chosen once, so the session is pinned to one model.
    pub async fn session(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
    ) -> Result<T::Session, ClientError> {
        Ok(self.transport.open(need, class, tier).await?)
    }

    /// [`Accounts::session`] with the options of `Open`: the caller's `traceparent`, so a span
    /// started here continues in inferd.
    pub async fn session_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<T::Session, ClientError> {
        Ok(self.transport.open_with(need, class, tier, options).await?)
    }

    /// Runs one AI request to its end on a fresh session and returns the reply, dropping the
    /// deltas; a refusal is an error the app shows.
    pub async fn infer(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        request: InferRequest,
    ) -> Result<InferReply, ClientError> {
        let mut session = self.session(need, class, tier).await?;
        // A daemon that refuses a session writes the refusal and hangs up, so the write can
        // fail while the answer is already waiting: read first, and report the write only
        // when there is nothing to read.
        let sent = session.send(ClientFrame::Request(request)).await;
        loop {
            match session.next().await {
                Ok(InferEvent::Finished(InferReply::Refused(refusal))) => {
                    return Err(ClientError::InferRefused(refusal));
                }
                Ok(InferEvent::Finished(reply)) => return Ok(reply),
                Ok(_) => {}
                Err(read) => {
                    let why = sent.err().unwrap_or(read);
                    return Err(crate::error::TransportError::from(why).into());
                }
            }
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
