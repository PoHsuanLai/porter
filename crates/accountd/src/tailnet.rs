//! Following Tailscale: the state of the Tailscale account, and the computers `Tailnet1` lists.
//!
//! accountd asks Tailscale (`porter-tailscale`, over its unix socket) and never keeps a copy of
//! the answer longer than a look: `Machines` reads Tailscale when it is called. What this module
//! keeps up to date is the account's state and the `Changed` signal.
//!
//! **Watch, with a poll behind it.** tailscaled serves a stream of notices
//! (`watch-ipn-bus`, newline-delimited JSON over the same socket) that says when its state or its
//! network changed, so accountd reads the stream and looks again at `status` on each notice. The
//! stream is only a nudge: it needs tailscaled to be running, and a change it did not announce
//! (a computer going offline is announced by a net map in some versions and not in others) would
//! otherwise be missed. So accountd also looks every half minute while the stream is open, and
//! every ten seconds tries to open the stream again while it is not (Tailscale stopped, not
//! started yet, or not letting porter ask).
//!
//! **What the state follows.** Running and signed in: `Ok`. Signed out: `NeedsLogin`, the state
//! that means "sign in again" (the person signs in in Tailscale; Settings offers "Sign in", which
//! hands them Tailscale's own page). Turned off, not running, not installed, or not letting
//! porter ask: `Offline`, the account kept. In the middle of starting: unchanged, since the next
//! answer settles it.

use crate::callers::Callers;
use crate::core::{Core, Host};
use porter_core::{AccountId, AccountState, AuthKind, Machine};
use porter_tailscale::{LocalApi, Standing, Status, TailscaleError};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// How accountd follows Tailscale: where it is, and how often it looks.
#[derive(Debug, Clone)]
pub struct TailnetWatch {
    /// Tailscale's socket.
    pub api: LocalApi,
    /// How often it looks when the notices are not coming (the stream is open, and silent).
    pub backstop: Duration,
    /// How long it waits before it tries the stream again, when there is none.
    pub retry: Duration,
    /// The shortest time between two `Changed` signals: changes in a burst are one signal.
    pub coalesce: Duration,
}

impl TailnetWatch {
    /// The usual timing, over `api`: a look every 30 seconds behind the stream, a new try every
    /// 10 seconds without one, and one signal a second at most.
    pub fn new(api: LocalApi) -> Self {
        Self {
            api,
            backstop: Duration::from_secs(30),
            retry: Duration::from_secs(10),
            coalesce: Duration::from_secs(1),
        }
    }

    /// This computer's own Tailscale, at the path it uses.
    pub fn system() -> Self {
        Self::new(LocalApi::system())
    }
}

/// The state Tailscale's answer puts the account in; none when the answer settles nothing.
pub(crate) fn state_of(answer: &Result<Status, TailscaleError>) -> Option<AccountState> {
    match answer {
        Ok(status) => match status.standing() {
            Standing::Ready => Some(AccountState::Ok),
            Standing::SignedOut => Some(AccountState::NeedsLogin),
            Standing::Down => Some(AccountState::Offline),
            Standing::Changing => None,
        },
        Err(
            TailscaleError::NotInstalled
            | TailscaleError::NotRunning
            | TailscaleError::Refused
            | TailscaleError::TimedOut,
        ) => Some(AccountState::Offline),
        // An answer porter cannot read says nothing about the account.
        Err(_) => None,
    }
}

/// The Tailscale account and its state, as the registry holds it now.
pub(crate) fn account_of<H: Host>(host: &H) -> Option<(AccountId, AccountState)> {
    host.registry()
        .accounts
        .into_iter()
        .find(|a| a.auth == AuthKind::OwnProgram)
        .map(|a| (a.id, a.state))
}

/// The computers `Tailnet1` lists now: those of Tailscale's answer, when there is a Tailscale
/// account that is working and Tailscale says it is signed in and running; else none.
pub(crate) async fn machines<H: Host>(host: &H, api: &LocalApi) -> Vec<Machine> {
    listed(host, &api.status().await)
}

/// [`machines`] over an answer already read.
fn listed<H: Host>(host: &H, answer: &Result<Status, TailscaleError>) -> Vec<Machine> {
    match (account_of(host), answer) {
        (Some((_, AccountState::Ok)), Ok(status)) if status.standing() == Standing::Ready => {
            status.machines()
        }
        _ => Vec::new(),
    }
}

/// Follows Tailscale until accountd stops: sets the account's state, and says `Changed`.
pub(crate) async fn follow<H: Host, C: Callers>(core: Arc<Core<H, C>>, watch: TailnetWatch) {
    // What the listeners were last told.
    let mut told: Vec<Machine> = Vec::new();
    let mut first = true;
    loop {
        let stream = watch.api.watch().await;
        let mut dirty: Option<Instant> = None;
        look(&core, &watch, &mut told, &mut first, &mut dirty).await;
        if let Ok(mut notices) = stream {
            loop {
                let due = dirty.map(|since| since + watch.coalesce);
                tokio::select! {
                    notice = notices.next() => match notice {
                        Ok(Some(_)) => look(&core, &watch, &mut told, &mut first, &mut dirty).await,
                        // The stream ended or broke: Tailscale stopped. Look once more, then retry.
                        Ok(None) | Err(_) => break,
                    },
                    () = tokio::time::sleep(watch.backstop) => {
                        look(&core, &watch, &mut told, &mut first, &mut dirty).await;
                    }
                    () = until(due) => say(&core, &watch, &mut told, &mut dirty).await,
                }
            }
            look(&core, &watch, &mut told, &mut first, &mut dirty).await;
        }
        // Not open (or just ended): wait out the retry, still telling what is due.
        let wake = Instant::now() + watch.retry;
        loop {
            let due = dirty.map(|since| since + watch.coalesce);
            tokio::select! {
                () = tokio::time::sleep_until(wake) => break,
                () = until(due) => say(&core, &watch, &mut told, &mut dirty).await,
            }
        }
    }
}

/// Waits for `at`; for ever when there is none.
async fn until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// One look at Tailscale: the account's state now, and whether the computers differ from what the
/// listeners were told (then the window of a `Changed` opens, if it is not open).
async fn look<H: Host, C: Callers>(
    core: &Arc<Core<H, C>>,
    watch: &TailnetWatch,
    told: &mut Vec<Machine>,
    first: &mut bool,
    dirty: &mut Option<Instant>,
) {
    let answer = watch.api.status().await;
    if let (Some((account, now)), Some(state)) = (account_of(&*core.host), state_of(&answer))
        && state != now
        && core.host.set_state(&account, state).await
    {
        core.publish().await;
    }
    let current = listed(&*core.host, &answer);
    if *first {
        // Nobody has been told anything yet: this is what they will read when they ask.
        *told = current;
        *first = false;
    } else if current != *told && dirty.is_none() {
        *dirty = Some(Instant::now());
    }
}

/// The window of a burst ends: say `Changed` once, if the computers still differ.
async fn say<H: Host, C: Callers>(
    core: &Arc<Core<H, C>>,
    watch: &TailnetWatch,
    told: &mut Vec<Machine>,
    dirty: &mut Option<Instant>,
) {
    *dirty = None;
    let current = machines(&*core.host, &watch.api).await;
    if current != *told {
        *told = current;
        crate::tailnet_object::announce(core).await;
    }
}
