//! What inferd tells accountd of a local runtime: `Peer.ReportLocal`, so the runtime is an
//! account (Ollama, llama.cpp, LM Studio; design/31 §3.3) that Settings lists and that goes
//! `Offline`, never gone, when the runtime stops. Only a porter daemon may call it, and it
//! carries no secret: the provider's id, the models as `Discovered` claims, and a state.
//!
//! [`Reports`] is the seam: the bus peer in the daemon, a recorder in a unit test.

use crate::cloud::accountd::Boxed;
use crate::probe::Runtime;
use crate::probed::Standing;
pub use porter_client::peer::claim_to_dbus;
use porter_client::peer::{LocalState, PeerAccounts, PeerError};
use porter_core::{AccountId, Claim};
use std::fmt::Debug;

/// Why accountd did not take a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFault {
    /// Not on the bus, or the call failed: ask again at the next look.
    Unreachable,
    /// accountd will not take it from this caller (not a porter daemon).
    Refused,
    /// accountd did not understand it (an unknown provider, a claim it does not accept).
    Rejected,
}

/// accountd, as inferd reports a runtime to it.
pub trait Reports: Debug + Send + Sync + 'static {
    /// Reports `runtime` in `standing` with its models as `claims`; the id of its account.
    fn report<'a>(
        &'a self,
        runtime: Runtime,
        claims: &'a [Claim],
        standing: Standing,
    ) -> Boxed<'a, Result<AccountId, ReportFault>>;
}

/// The state porter-client tells accountd for a standing.
fn local_state(standing: Standing) -> LocalState {
    match standing {
        Standing::Online => LocalState::Ok,
        Standing::Offline => LocalState::Offline,
    }
}

/// The state's slug on the bus.
pub fn state_slug(standing: Standing) -> &'static str {
    local_state(standing).slug()
}

impl From<&PeerError> for ReportFault {
    /// What inferd does about it: a refusal and a rejection are accountd's answer, and any other
    /// failure is "ask again at the next look".
    fn from(error: &PeerError) -> Self {
        match error {
            PeerError::Denied(_) => ReportFault::Refused,
            PeerError::Rejected(_) | PeerError::Malformed(_) => ReportFault::Rejected,
            _ => ReportFault::Unreachable,
        }
    }
}

impl From<PeerError> for ReportFault {
    fn from(error: PeerError) -> Self {
        ReportFault::from(&error)
    }
}

/// accountd over the session bus: `org.quire.Accounts1.Peer.ReportLocal`.
#[derive(Debug, Clone)]
pub struct PeerReports {
    peer: PeerAccounts,
}

impl PeerReports {
    /// Reports over `connection`.
    pub fn new(connection: zbus::Connection) -> Self {
        Self {
            peer: PeerAccounts::over(&connection),
        }
    }
}

impl Reports for PeerReports {
    fn report<'a>(
        &'a self,
        runtime: Runtime,
        claims: &'a [Claim],
        standing: Standing,
    ) -> Boxed<'a, Result<AccountId, ReportFault>> {
        Box::pin(async move {
            Ok(self
                .peer
                .report_local(runtime.provider(), claims, local_state(standing))
                .await?)
        })
    }
}

/// No accountd: every report is `Unreachable`, so the daemon serves its probed runtimes on its
/// own and tells accountd when there is one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoReports;

impl Reports for NoReports {
    fn report<'a>(
        &'a self,
        _: Runtime,
        _: &'a [Claim],
        _: Standing,
    ) -> Boxed<'a, Result<AccountId, ReportFault>> {
        Box::pin(async { Err(ReportFault::Unreachable) })
    }
}

#[cfg(test)]
mod tests;
