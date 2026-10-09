//! inferd across the person's Tailscale network.
//!
//! The logic is in `porter_tailnet` (who may ask, the answers the person gave, the listeners,
//! the dial, the relay); this module is the wiring to inferd's own parts:
//!
//! * **Lending.** While `ai.tailnet.serve` is on, [`Tailnet`] follows Tailscale and listens on
//!   this computer's network addresses at the one port; every connection is judged by
//!   `porter_tailnet::judge` and served by [`Lender`] (the computer's hello, the models this
//!   computer runs, chat). While it is off nothing is listened on and Tailscale is not even
//!   asked.
//! * **Calling.** [`Candidates`] looks at the person's other computers that lend (when somebody
//!   reads the list), and a computer that is added is reached through a relay at a socket of
//!   inferd's own (`relay_socket`): an attached engine like any other, whose connections ask
//!   Tailscale who is at the address first.
//! * **Guests.** The computers that ask to use this one, and the person's answers, are
//!   `porter_tailnet::Guests`, kept in a file of inferd's own; [`Tailnet::subscribe`] is how the
//!   bus is told that one is asking.

mod candidates;
mod lender;
mod machines;

pub use candidates::{Candidate, Candidates};
pub use lender::Lender;
pub use machines::{BusMachines, Machines, NoMachines};

use crate::attached::relay_socket;
use crate::audit::AuditOut;
use crate::clock::Clock;
use crate::engines::Engines;
use crate::settings::TailnetServe;
use porter_core::NodeId;
use porter_tailnet::{
    Config, Dialer, GuestEvent, Guests, Lending, Listening, Observer, OnFault, Relay, Timing, lend,
};
use porter_tailscale::LocalApi;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

/// What [`Tailnet::start`] is given.
#[derive(Debug, Clone)]
pub struct Parts {
    /// This computer's Tailscale.
    pub api: LocalApi,
    /// The person's other computers, as accountd lists them.
    pub machines: Arc<dyn Machines>,
    /// The answers and the questions waiting.
    pub guests: Arc<Guests>,
    /// Where the relays' sockets go.
    pub sockets: PathBuf,
    /// The port, the addresses allowed and the limits of listening.
    pub config: Config,
    /// How Tailscale is followed while lending.
    pub timing: Timing,
}

struct Running {
    // Held for as long as lending is on; dropping them stops it.
    _observer: Observer,
    _lending: Lending,
    _forward: JoinHandle<()>,
    _on: watch::Sender<bool>,
}

struct Inner {
    api: LocalApi,
    dialer: Dialer,
    candidates: Candidates,
    machines: Arc<dyn Machines>,
    guests: Arc<Guests>,
    sockets: PathBuf,
    relays: Mutex<BTreeMap<NodeId, Relay>>,
    events: broadcast::Sender<GuestEvent>,
    listening: watch::Sender<Listening>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = self
            .supervisor
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

/// inferd's side of the Tailscale network, shared by every clone.
#[derive(Clone)]
pub struct Tailnet(Arc<Inner>);

impl std::fmt::Debug for Tailnet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tailnet").finish_non_exhaustive()
    }
}

impl Tailnet {
    /// Starts following `engines`' settings: lending is switched on and off with
    /// `ai.tailnet.serve`. Nothing is asked of Tailscale until it is on, or a computer is
    /// reached.
    pub fn start(
        parts: Parts,
        engines: Engines,
        audit: Arc<dyn AuditOut>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let dialer = Dialer::new(parts.api.clone(), parts.config.port);
        let (events, _) = broadcast::channel(64);
        let told = events.clone();
        parts.guests.on_event(move |event| {
            // Nobody listening is fine: a shell that is not running reads `Guests` when it starts.
            let _ = told.send(event.clone());
        });
        let (listening, _) = watch::channel(Listening::default());
        let inner = Arc::new(Inner {
            candidates: Candidates::new(dialer.clone(), Arc::clone(&parts.machines)),
            api: parts.api.clone(),
            dialer,
            machines: Arc::clone(&parts.machines),
            guests: Arc::clone(&parts.guests),
            sockets: parts.sockets.clone(),
            relays: Mutex::default(),
            events,
            listening,
            supervisor: Mutex::new(None),
        });
        let lender = Arc::new(Lender::new(engines.clone(), parts.guests, audit, clock));
        let task = tokio::spawn(supervise(
            Arc::downgrade(&inner),
            engines,
            lender,
            parts.api,
            parts.config,
            parts.timing,
        ));
        *inner
            .supervisor
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(task);
        Self(inner)
    }

    /// The answers and the questions waiting.
    pub fn guests(&self) -> &Arc<Guests> {
        &self.0.guests
    }

    /// News of the computers that ask: a question raised, or the answers changed.
    pub fn subscribe(&self) -> broadcast::Receiver<GuestEvent> {
        self.0.events.subscribe()
    }

    /// Where listening stands (nothing while lending is off).
    pub fn listening(&self) -> watch::Receiver<Listening> {
        self.0.listening.subscribe()
    }

    /// The person's other computers, as accountd lists them.
    pub fn machines(&self) -> &Arc<dyn Machines> {
        &self.0.machines
    }

    /// The computers that lend and could be added; the ones due are looked at now.
    pub async fn candidates(&self) -> Vec<Candidate> {
        self.0.candidates.list().await
    }

    /// Starts the relay that leads to the computer `node`, if there is none: the socket an
    /// engine on it is reached at.
    pub fn ensure_relay(&self, node: &NodeId) -> std::io::Result<()> {
        let mut relays = self.0.relays.lock().unwrap_or_else(PoisonError::into_inner);
        if relays.contains_key(node) {
            return Ok(());
        }
        let path = relay_socket(&self.0.sockets, node).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "the path is too long")
        })?;
        let told: OnFault = {
            let node = node.clone();
            Arc::new(move |fault| eprintln!("inferd: computer {node}: {fault}"))
        };
        let relay = Relay::start(&path, node.clone(), self.0.dialer.clone(), told)?;
        relays.insert(node.clone(), relay);
        Ok(())
    }

    /// Stops the relay of `node`.
    pub fn drop_relay(&self, node: &NodeId) {
        self.0
            .relays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(node);
    }

    /// This computer's Tailscale.
    pub fn api(&self) -> &LocalApi {
        &self.0.api
    }
}

/// Switches lending on and off with the setting.
async fn supervise(
    inner: std::sync::Weak<Inner>,
    engines: Engines,
    lender: Arc<Lender>,
    api: LocalApi,
    config: Config,
    timing: Timing,
) {
    let mut settings = engines.watch_settings();
    let mut running: Option<Running> = None;
    loop {
        let on = settings.borrow().tailnet_serve == TailnetServe::On;
        let Some(inner) = inner.upgrade() else { return };
        match (on, running.is_some()) {
            (true, false) => {
                let observer = Observer::start(api.clone(), timing);
                let (switch, on_rx) = watch::channel(true);
                let lending = lend(
                    api.clone(),
                    observer.seen(),
                    on_rx,
                    Arc::clone(&inner.guests),
                    Arc::clone(&lender),
                    config,
                );
                let mut state = lending.listening();
                let told = inner.listening.clone();
                let forward = tokio::spawn(async move {
                    loop {
                        told.send_replace(state.borrow().clone());
                        if state.changed().await.is_err() {
                            return;
                        }
                    }
                });
                running = Some(Running {
                    _observer: observer,
                    _lending: lending,
                    _forward: forward,
                    _on: switch,
                });
            }
            (false, true) => {
                if let Some(stopped) = running.take() {
                    stopped._forward.abort();
                }
                inner.listening.send_replace(Listening::default());
            }
            _ => {}
        }
        drop(inner);
        if settings.changed().await.is_err() {
            return;
        }
    }
}
