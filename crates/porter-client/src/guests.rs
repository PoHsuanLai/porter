//! `GuestAsks` and `GuestsChanged`: hearing from inferd that a computer of the person's began
//! asking to use this computer's models, or that the answers changed, so Settings and the
//! shell can ask the person ("Let Studio use this computer's models?") without polling.
//!
//! ```ignore
//! let mut changes = accounts.watch_guests().await?;   // subscribe before reading the list
//! for row in accounts.guests().await? {               // GuestRow { node, name, state, since }
//!     /* show it; a row in state Asking needs an answer */
//! }
//! while let Some(change) = changes.next().await {
//!     match change {
//!         GuestChange::Asks { node, ask } => { /* ask the person about ask.name */ }
//!         GuestChange::Changed => { /* read the list again */ }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! Both signals are broadcast by inferd, so there is nothing to call first. A shell that was
//! not running when a computer began asking reads it from `guests()` (state `Asking`).

use crate::error::TransportError;
use crate::transport::bus_error;
use porter_core::lending::GuestAsk;
use porter_core::{NodeId, UnixSeconds};
use porter_dbus::{
    BusConnection, BusStream, Details, GUEST_KEY_NAME, GUEST_KEY_SINCE, GuestAsksStream,
    GuestsChangedStream, InferenceProxy,
};
use std::pin::Pin;
use std::task::{Context, Poll};

/// One thing heard about the computers that ask. More may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuestChange {
    /// A computer began asking: the person is to be asked now. A signal that does not say the
    /// computer's id and name plainly is [`GuestChange::Changed`] instead, so the list is read
    /// again and shows the question.
    Asks {
        /// Its stable id: what `answer_guest` takes.
        node: NodeId,
        /// Its name and when it began asking.
        ask: GuestAsk,
    },
    /// The answers or the questions waiting changed: read the list again.
    Changed,
}

/// The guest changes from now on. Dropping it stops the watch.
#[derive(Debug)]
pub struct GuestChanges {
    asks: GuestAsksStream,
    changed: GuestsChangedStream,
}

impl GuestChanges {
    /// Subscribes to both signals of inferd.
    pub(crate) async fn watch(connection: &BusConnection) -> Result<Self, TransportError> {
        let proxy = InferenceProxy::new(connection)
            .await
            .map_err(|e| bus_error(&e))?;
        Ok(Self {
            asks: proxy
                .receive_guest_asks()
                .await
                .map_err(|e| bus_error(&e))?,
            changed: proxy
                .receive_guests_changed()
                .await
                .map_err(|e| bus_error(&e))?,
        })
    }

    /// The next change, or `None` when the connection has ended.
    pub async fn next(&mut self) -> Option<GuestChange> {
        std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await
    }
}

impl BusStream for GuestChanges {
    type Item = GuestChange;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        // The questions first: a person waiting for one is the more urgent.
        if let Poll::Ready(signal) = Pin::new(&mut this.asks).poll_next(cx) {
            return Poll::Ready(signal.map(|signal| {
                signal
                    .args()
                    .ok()
                    .and_then(|args| ask_of(args.node(), args.details()))
                    .unwrap_or(GuestChange::Changed)
            }));
        }
        Pin::new(&mut this.changed)
            .poll_next(cx)
            .map(|next| next.map(|_| GuestChange::Changed))
    }
}

/// What a `GuestAsks` signal says; none when the id or the name is not plain.
fn ask_of(node: &str, details: &Details) -> Option<GuestChange> {
    let name = String::try_from(details.get(GUEST_KEY_NAME)?.try_clone().ok()?).ok()?;
    let since = i64::try_from(details.get(GUEST_KEY_SINCE)?).ok()?;
    Some(GuestChange::Asks {
        node: NodeId::parse(node).ok()?,
        ask: GuestAsk::new(name, UnixSeconds(since)),
    })
}
