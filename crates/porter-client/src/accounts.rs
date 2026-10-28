//! `Accounts`: the app's handle on porter.

use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::error::ClientError;
#[cfg(feature = "dbus")]
use crate::error::TransportError;
use crate::found::ConsentOffer;
use crate::transport::Transport;
#[cfg(feature = "infer")]
use porter_capability::Tier;
#[cfg(feature = "infer")]
use porter_core::DataClass;
use porter_core::ProviderHint;
use porter_core::{AccountId, GrantId};
use porter_core::{Candidate, EndpointUrl};
#[cfg(feature = "infer")]
use porter_infer::{
    ClientFrame, InferEvent, InferReply, InferRequest, InferSession, OpenOptions, Readiness,
};
#[cfg(feature = "infer")]
use porter_match::Need;
use porter_tailnet::{ComputerCandidate, GuestAnswer, GuestRow};
use porter_tailscale::NodeId;
use porter_wire::ParentWindow;
use porter_wire::{AccountsReply, AccountsRequest};

/// The app's connection to the account service.
#[derive(Debug)]
pub struct Accounts<T> {
    pub(crate) transport: T,
}

#[cfg(feature = "dbus")]
impl Accounts<crate::transport::DbusTransport> {
    /// Connects over the session bus. The daemons are found, and started by bus activation, at
    /// the first call, so a machine with inferd and no accountd still connects. No session bus is
    /// `TransportError::Unreachable`.
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

    /// The computers that ask to use this computer's models, from now on: a question the person
    /// is to answer, and every change to the answers. Subscribe before reading
    /// [`Accounts::guests`], so no change falls between the two.
    pub async fn watch_guests(&self) -> Result<crate::GuestChanges, ClientError> {
        Ok(
            crate::GuestChanges::watch(self.transport.connection(), self.transport.inference())
                .await?,
        )
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

    /// Opens a streaming session with inferd: write `ClientFrame`s, read `InferEvent`s. The
    /// route is chosen once, so the session is pinned to one model.
    #[cfg(feature = "infer")]
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
    #[cfg(feature = "infer")]
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
    #[cfg(feature = "infer")]
    pub async fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<Readiness, ClientError> {
        Ok(self.transport.prepare(need, class, tier, options).await?)
    }

    /// The places the assistant could run, one row each ([`Transport::places`]). Only Settings,
    /// the shell and the companion may ask.
    #[cfg(feature = "infer")]
    pub async fn places(&self) -> Result<Vec<porter_infer::PlaceRow>, ClientError> {
        Ok(self.transport.places().await?)
    }

    /// The computers that asked to use this computer's models, or were answered, one row each
    /// ([`Transport::guests`]). Only Settings and the shell may ask; a link with no inferd on a
    /// bus is `TransportError::Unsupported`.
    pub async fn guests(&self) -> Result<Vec<GuestRow>, ClientError> {
        Ok(self.transport.guests().await?)
    }

    /// The person's answer about the computer `node`: let it use this computer's models, or keep
    /// it out ([`Transport::answer_guest`]). The shell may answer a computer that is asking;
    /// Settings may answer any computer of the person's.
    pub async fn answer_guest(
        &self,
        node: &NodeId,
        answer: GuestAnswer,
    ) -> Result<(), ClientError> {
        Ok(self.transport.answer_guest(node, answer).await?)
    }

    /// Forgets the answer about `node` and any question it has waiting, so it is asked about
    /// again ([`Transport::forget_guest`]). Settings only.
    pub async fn forget_guest(&self, node: &NodeId) -> Result<(), ClientError> {
        Ok(self.transport.forget_guest(node).await?)
    }

    /// The person's own computers on their Tailscale network that lend their models and are not
    /// added yet ([`Transport::candidates`]). Settings only.
    pub async fn candidates(&self) -> Result<Vec<ComputerCandidate>, ClientError> {
        Ok(self.transport.candidates().await?)
    }

    /// Adds the candidate `node` as one of the person's own computers and answers its place
    /// ([`Transport::add_tailnet_computer`]). Settings only.
    #[cfg(feature = "infer")]
    pub async fn add_tailnet_computer(
        &self,
        node: &NodeId,
    ) -> Result<porter_infer::PlaceId, ClientError> {
        Ok(self.transport.add_tailnet_computer(node).await?)
    }

    /// Adds a computer of the person's own by hand and answers its place
    /// ([`Transport::add_computer`]). Settings only.
    #[cfg(feature = "infer")]
    pub async fn add_computer(
        &self,
        computer: &crate::NewComputer,
    ) -> Result<porter_infer::PlaceId, ClientError> {
        Ok(self.transport.add_computer(computer).await?)
    }

    /// Removes a computer that was added ([`Transport::remove_computer`]). Settings only.
    #[cfg(feature = "infer")]
    pub async fn remove_computer(
        &self,
        name: &porter_infer::ComputerName,
    ) -> Result<(), ClientError> {
        Ok(self.transport.remove_computer(name).await?)
    }

    /// Sets the models Automatic chooses among for the helper `helper` in `slot`
    /// ([`Transport::set_helper_pool`]); each must be a model inferd knows for the slot, and an
    /// empty list takes the helper off Automatic for it. Settings and the shell may call.
    #[cfg(feature = "infer")]
    pub async fn set_helper_pool(
        &self,
        slot: porter_infer::Slot,
        helper: &porter_core::HelperId,
        models: &[porter_infer::ModelRef],
    ) -> Result<(), ClientError> {
        Ok(self.transport.set_helper_pool(slot, helper, models).await?)
    }

    /// Runs one AI request to its end on a fresh session and returns the reply, dropping the
    /// deltas; a refusal is an error the app shows.
    #[cfg(feature = "infer")]
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
