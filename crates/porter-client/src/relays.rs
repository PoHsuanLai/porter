//! The host that runs relays for an app hosting porter in process: the app owns the runtime and
//! the network, so it spawns the relay (`porter-proxy`'s `relay` over a TLS connector it
//! chooses) and the client only hands it the plan and the relay's end of an in-memory duplex.

use porter_core::RelayPlan;
use porter_core::stream::DuplexEnd;

/// Runs the relay for a plan the service checked.
pub trait RelayHost: Send + Sync {
    /// Starts a relay for `plan` that talks to the app through `end`, and returns at once. The
    /// relay ends when either side finishes.
    fn run(&self, plan: RelayPlan, end: DuplexEnd);
}

/// No relays hosted: opening an authenticated stream says nobody is reachable, as a daemon that
/// is not running does.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRelays;

impl RelayHost for NoRelays {
    fn run(&self, plan: RelayPlan, end: DuplexEnd) {
        let _ = (plan, end);
    }
}
