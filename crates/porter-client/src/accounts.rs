//! `Accounts`: the app's handle on porter.

use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::env::{ClientEnv, LinkChoice};
use crate::error::{ClientError, TransportError};
use crate::found::{ConsentOffer, Found, found};
use crate::transport::{AnyTransport, Transport};
use porter_core::consent::{Grant, Usage};
use porter_core::wire::LegacyRef;
use porter_core::wire::{ParentWindow, ProviderHint};
use porter_core::{
    AccountId, AccountsReply, AccountsRequest, Audience, Candidate, DataClass, EndpointUrl,
    GrantId, IssuedToken, Need, Tier,
};
use porter_infer::{
    ClientFrame, InferEvent, InferReply, InferRequest, InferSession, OpenOptions, Readiness,
};

/// The app's connection to the account service.
#[derive(Debug)]
pub struct Accounts<T> {
    transport: T,
}

impl Accounts<AnyTransport> {
    /// Connects over the first reachable link in `env`, in order (D-Bus on the desktop, the
    /// latchkey socket elsewhere). A D-Bus link is reachable when the session bus is: the
    /// daemons are found, and started by bus activation, at the first call, so a machine with
    /// inferd and no accountd still connects. A socket link is reachable when the agent accepts
    /// a connection. Without the `dbus` feature a D-Bus link is skipped, and without `socket` a
    /// socket link is; no reachable link is `TransportError::Unreachable`.
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
        LinkChoice::Socket(path) => {
            let link = crate::transport::SocketTransport::at(path.clone());
            link.reachable().await.then_some(AnyTransport::Socket(link))
        }
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

    /// Brings the app's own earlier account in as an account of porter (mailo's keyring entries
    /// become a porter account): `legacy` names it by non-secret facts and accountd reads the
    /// old secret items itself, so no credential crosses the transport.
    pub async fn adopt(&self, legacy: LegacyRef) -> Result<AccountId, ClientError> {
        match self
            .transport
            .call(AccountsRequest::Adopt { legacy })
            .await?
        {
            AccountsReply::Adopted(account) => Ok(account),
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

    /// Warms the engine inferd would route `need` to and says how ready it is
    /// ([`Transport::prepare`]): a hold that is about to begin asks first, so the model is
    /// loading while the person starts to talk.
    pub async fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<Readiness, ClientError> {
        Ok(self.transport.prepare(need, class, tier, options).await?)
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
