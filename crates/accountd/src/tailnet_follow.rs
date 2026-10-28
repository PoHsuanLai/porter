//! Following Tailscale until accountd stops: the account's state, and `Tailnet1`'s `Changed`.
//! What the state follows, and how often accountd looks, is `tailnet`'s.

use crate::callers::Callers;
use crate::host::Host;
use crate::state::Core;
use crate::tailnet::{TailnetWatch, account_of, listed, machines, state_of};
use porter_tailscale::Machine;
use std::sync::Arc;
use tokio::time::Instant;

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
