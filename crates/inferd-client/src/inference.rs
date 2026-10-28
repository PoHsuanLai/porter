//! `Inference`: the app's handle on inferd.

use crate::error::{ClientError, TransportError};
use crate::transport::Transport;
use porter_capability::Tier;
use porter_core::DataClass;
use porter_infer::{
    ClientFrame, InferEvent, InferReply, InferRequest, InferSession, OpenOptions, Readiness,
};
use porter_match::Need;
use porter_tailnet::{ComputerCandidate, GuestAnswer, GuestRow};
use porter_tailscale::NodeId;

/// The app's connection to inferd.
#[derive(Debug)]
pub struct Inference<T> {
    pub(crate) transport: T,
}

#[cfg(feature = "dbus")]
impl Inference<crate::transport::DbusTransport> {
    /// Connects over the session bus. inferd is found, and started by bus activation, at the
    /// first call. No session bus is `TransportError::Unreachable`.
    pub async fn connect() -> Result<Self, ClientError> {
        crate::transport::DbusTransport::session()
            .await
            .map(|transport| Self { transport })
            .map_err(|_| TransportError::Unreachable.into())
    }

    /// The computers that ask to use this computer's models, from now on: a question the person
    /// is to answer, and every change to the answers. Subscribe before reading
    /// [`Inference::guests`], so no change falls between the two.
    pub async fn watch_guests(&self) -> Result<crate::GuestChanges, ClientError> {
        Ok(
            crate::GuestChanges::watch(self.transport.connection(), self.transport.inference())
                .await?,
        )
    }
}

impl<T: Transport> Inference<T> {
    /// Over a transport the app built (`Brokered` for an app hosting the routing).
    pub fn over(transport: T) -> Self {
        Self { transport }
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

    /// [`Inference::session`] with the options of `Open`: the caller's `traceparent`, so a span
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

    /// The places the assistant could run, one row each ([`Transport::places`]). Only Settings,
    /// the shell and the companion may ask.
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
    pub async fn add_tailnet_computer(
        &self,
        node: &NodeId,
    ) -> Result<porter_infer::PlaceId, ClientError> {
        Ok(self.transport.add_tailnet_computer(node).await?)
    }

    /// Adds a computer of the person's own by hand and answers its place
    /// ([`Transport::add_computer`]). Settings only.
    pub async fn add_computer(
        &self,
        computer: &crate::NewComputer,
    ) -> Result<porter_infer::PlaceId, ClientError> {
        Ok(self.transport.add_computer(computer).await?)
    }

    /// Removes a computer that was added ([`Transport::remove_computer`]). Settings only.
    pub async fn remove_computer(
        &self,
        name: &porter_infer::ComputerName,
    ) -> Result<(), ClientError> {
        Ok(self.transport.remove_computer(name).await?)
    }

    /// Sets the models Automatic chooses among for the helper `helper` in `slot`
    /// ([`Transport::set_helper_pool`]); each must be a model inferd knows for the slot, and an
    /// empty list takes the helper off Automatic for it. Settings and the shell may call.
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
                    return Err(TransportError::from(why).into());
                }
            }
        }
    }
}
