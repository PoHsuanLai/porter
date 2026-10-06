//! What inferd tells accountd of a local runtime: `Peer.ReportLocal`, so the runtime is an
//! account (Ollama, llama.cpp, LM Studio; design/31 §3.3) that Settings lists and that goes
//! `Offline`, never gone, when the runtime stops. Only a porter daemon may call it, and it
//! carries no secret: the provider's id, the models as `Discovered` claims, and a state.
//!
//! [`Reports`] is the seam: the bus peer in the daemon, a recorder in a unit test.

use crate::cloud::accountd::Boxed;
use crate::probe::Runtime;
use crate::probed::Standing;
use porter_core::{AccountId, Claim};
use porter_dbus::{Details, PeerProxy, to_vardict};
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

/// The state's slug on the bus.
pub fn state_slug(standing: Standing) -> &'static str {
    match standing {
        Standing::Online => "ok",
        Standing::Offline => "offline",
    }
}

/// A claim as the bus carries it: the kind's slug and the claim's fields by name, as
/// `Account.Capabilities` has them.
pub fn claim_to_dbus(claim: &Claim) -> Option<(String, Details)> {
    let kind = serde_json::to_value(claim.offer.kind()).ok()?;
    let fields = match serde_json::to_value(claim).ok()? {
        serde_json::Value::Object(fields) => fields,
        _ => return None,
    };
    Some((kind.as_str()?.to_owned(), to_vardict(&fields)))
}

fn fault_of(error: &zbus::Error) -> ReportFault {
    match error {
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "org.freedesktop.DBus.Error.AccessDenied" => ReportFault::Refused,
            "org.freedesktop.DBus.Error.InvalidArgs" => ReportFault::Rejected,
            _ => ReportFault::Unreachable,
        },
        zbus::Error::FDO(fdo) => match **fdo {
            zbus::fdo::Error::AccessDenied(_) => ReportFault::Refused,
            zbus::fdo::Error::InvalidArgs(_) => ReportFault::Rejected,
            _ => ReportFault::Unreachable,
        },
        _ => ReportFault::Unreachable,
    }
}

/// accountd over the session bus: `org.quire.Accounts1.Peer.ReportLocal`.
#[derive(Debug, Clone)]
pub struct PeerReports {
    connection: zbus::Connection,
}

impl PeerReports {
    /// Reports over `connection`.
    pub fn new(connection: zbus::Connection) -> Self {
        Self { connection }
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
            let peer = PeerProxy::new(&self.connection)
                .await
                .map_err(|e| fault_of(&e))?;
            let claims: Vec<_> = claims.iter().filter_map(claim_to_dbus).collect();
            let id = peer
                .report_local(runtime.provider(), claims, state_slug(standing))
                .await
                .map_err(|e| fault_of(&e))?;
            AccountId::parse(&id).map_err(|_| ReportFault::Rejected)
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
