//! The carrier seam: one request in, one reply out. The caller's identity is the transport's
//! to establish, never the request's.
//!
//! One trait for accounts and inference. Inference is the feature `infer`: its items (`Session`,
//! `open_with`, `prepare`, `open`) exist only with it, so a build without it has a `Transport`
//! with the accounts calls alone. A separate `InferTransport` trait would have broken every
//! consumer that names `Transport` for `open` and `prepare` (docket does, on thirty call sites);
//! the cfg'd items break nobody, and only an implementor outside this crate that builds with
//! `infer` has to give a `Session`, as it did before.

mod broker;
#[cfg(feature = "dbus")]
mod dbus;
#[cfg(feature = "dbus")]
mod dbus_accounts;
#[cfg(feature = "dbus")]
mod dbus_computers;
#[cfg(all(feature = "dbus", feature = "infer"))]
mod dbus_infer;
#[cfg(all(feature = "dbus", feature = "infer"))]
mod dbus_session;
#[cfg(all(unix, feature = "dbus", feature = "infer"))]
mod framed;
#[cfg(feature = "in-process")]
mod in_process;

pub use broker::NoBroker;
#[cfg(feature = "infer")]
pub use broker::{InProcessSession, SessionHost};
#[cfg(feature = "dbus")]
pub(crate) use dbus::bus_error;
#[cfg(feature = "dbus")]
pub use dbus::{DbusTransport, Rows};
#[cfg(all(feature = "dbus", feature = "infer"))]
pub use dbus_session::{DbusSession, MAX_ATTACHMENTS};
#[cfg(feature = "in-process")]
pub use in_process::{HostedCore, InProcess};

use crate::authenticated::Relayed;
#[cfg(feature = "infer")]
use crate::computers::NewComputer;
use crate::error::TransportError;
use porter_core::GrantId;
use porter_core::lending::{ComputerCandidate, GuestAnswer, GuestRow};
#[cfg(feature = "infer")]
use porter_core::{DataClass, Tier};
use porter_core::{EndpointUrl, NodeId};
#[cfg(feature = "infer")]
use porter_infer::{ComputerName, InferSession, OpenOptions, PlaceId, PlaceRow, Readiness};
#[cfg(feature = "infer")]
use porter_match::Need;
use porter_wire::{AccountsReply, AccountsRequest};
use std::future::Future;

/// Carries requests to accountd and inferd.
pub trait Transport: Send + Sync {
    /// The streaming session `open` returns.
    #[cfg(feature = "infer")]
    type Session: InferSession;

    /// One request to accountd.
    fn call(
        &self,
        request: AccountsRequest,
    ) -> impl Future<Output = Result<AccountsReply, TransportError>> + Send;

    /// A byte stream to one endpoint of a granted account, authenticated by a daemon-side relay
    /// (`Tokens.OpenAuthenticated`). The descriptor is out of band, so it is not a `call`. A
    /// transport that cannot carry one is `Unreachable`.
    fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<Relayed, TransportError>> + Send {
        let _ = (grant, endpoint);
        async { Err(TransportError::Unreachable) }
    }

    /// A byte stream to `origin`, a host the account's provider file declares for the grant's
    /// kind as one its pre-authenticated links point at (`Tokens.OpenLinked`); the relay adds no
    /// credential. The descriptor is out of band. A transport that cannot carry one is
    /// `Unreachable`.
    fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<Relayed, TransportError>> + Send {
        let _ = (grant, origin);
        async { Err(TransportError::Unreachable) }
    }

    /// Opens a session with inferd for `need`, `class` and `tier`: the route is chosen once, so
    /// the session is pinned to one model. A refusal arrives as the session's first event
    /// (`Finished(Refused(..))`). `options` carries the caller's `traceparent` (the bus
    /// `options` dictionary) so one task is one trace.
    #[cfg(feature = "infer")]
    fn open_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> impl Future<Output = Result<Self::Session, TransportError>> + Send;

    /// Warms the engine inferd would pick for `need`, `class` and `tier` (no request, no
    /// microphone) and answers how ready it is (`Inference1.Prepare`). The wait is the caller's
    /// next `open`. A model that is not yet there is `Loadable` or `Downloadable`, one that
    /// cannot be served is `Unavailable`; any other refusal (no grant, denied, a spend cap, a
    /// class that may not leave the machine) is `TransportError::Denied` with its slug.
    ///
    /// The default is `Unreachable`, so a transport that has no inferd (and every implementor
    /// outside this crate) compiles and degrades as it does when inferd is not running.
    #[cfg(feature = "infer")]
    fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> impl Future<Output = Result<Readiness, TransportError>> + Send {
        let _ = (need, class, tier, options);
        async { Err(TransportError::Unreachable) }
    }

    /// The places the assistant could run, one row each (`Inference1.Places`). Only Settings, the
    /// shell and the companion may ask; anyone else is `Denied`.
    ///
    /// The default is `Unreachable`, as `prepare`'s is.
    #[cfg(feature = "infer")]
    fn places(&self) -> impl Future<Output = Result<Vec<PlaceRow>, TransportError>> + Send {
        async { Err(TransportError::Unreachable) }
    }

    /// The computers that asked to use this computer's models, or were answered, one row each
    /// (`Inference1.Guests`). Only Settings and the shell may ask.
    ///
    /// The default (and the answer of a link with no inferd on a bus: the app hosting the
    /// core itself) is [`TransportError::Unsupported`].
    fn guests(&self) -> impl Future<Output = Result<Vec<GuestRow>, TransportError>> + Send {
        async { Err(TransportError::Unsupported) }
    }

    /// The person's answer about the computer `node` (`Inference1.AnswerGuest`; on the bus the
    /// answer is the word `allow` or `deny`). The default is `Unsupported`.
    fn answer_guest(
        &self,
        node: &NodeId,
        answer: GuestAnswer,
    ) -> impl Future<Output = Result<(), TransportError>> + Send {
        let _ = (node, answer);
        async { Err(TransportError::Unsupported) }
    }

    /// Forgets the answer about `node`, and any question it has waiting
    /// (`Inference1.ForgetGuest`; Settings only). The default is `Unsupported`.
    fn forget_guest(
        &self,
        node: &NodeId,
    ) -> impl Future<Output = Result<(), TransportError>> + Send {
        let _ = node;
        async { Err(TransportError::Unsupported) }
    }

    /// The person's own computers on their Tailscale network that lend their models and are not
    /// added yet (`Inference1.Candidates`; Settings only). The default is `Unsupported`.
    fn candidates(
        &self,
    ) -> impl Future<Output = Result<Vec<ComputerCandidate>, TransportError>> + Send {
        async { Err(TransportError::Unsupported) }
    }

    /// Adds the candidate `node` as one of the person's own computers and answers its place
    /// (`Inference1.AddTailnetComputer`; Settings only). A refusal is
    /// [`TransportError::Computer`]. The default is `Unsupported`.
    #[cfg(feature = "infer")]
    fn add_tailnet_computer(
        &self,
        node: &NodeId,
    ) -> impl Future<Output = Result<PlaceId, TransportError>> + Send {
        let _ = node;
        async { Err(TransportError::Unsupported) }
    }

    /// Adds a computer of the person's own by hand and answers its place
    /// (`Inference1.AddComputer`; Settings only). The default is `Unsupported`.
    #[cfg(feature = "infer")]
    fn add_computer(
        &self,
        computer: &NewComputer,
    ) -> impl Future<Output = Result<PlaceId, TransportError>> + Send {
        let _ = computer;
        async { Err(TransportError::Unsupported) }
    }

    /// Removes a computer that `add_computer` or `add_tailnet_computer` added
    /// (`Inference1.RemoveComputer`; Settings only). The default is `Unsupported`.
    #[cfg(feature = "infer")]
    fn remove_computer(
        &self,
        name: &ComputerName,
    ) -> impl Future<Output = Result<(), TransportError>> + Send {
        let _ = name;
        async { Err(TransportError::Unsupported) }
    }

    /// Sets the models Automatic chooses among for the helper `helper` in `slot`
    /// (`Inference1.SetHelperPool`; Settings and the shell). An empty list takes the helper off
    /// Automatic for the slot. The default is `Unsupported`.
    #[cfg(feature = "infer")]
    fn set_helper_pool(
        &self,
        slot: porter_infer::Slot,
        helper: &porter_core::HelperId,
        models: &[porter_infer::ModelRef],
    ) -> impl Future<Output = Result<(), TransportError>> + Send {
        let _ = (slot, helper, models);
        async { Err(TransportError::Unsupported) }
    }

    /// [`Transport::open_with`] with no trace context: inferd starts its own root.
    #[cfg(feature = "infer")]
    fn open(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
    ) -> impl Future<Output = Result<Self::Session, TransportError>> + Send {
        async move {
            self.open_with(need, class, tier, &OpenOptions::default())
                .await
        }
    }
}
