//! What inferd tells its own key store of a local runtime: the runtime is an account (Ollama,
//! llama.cpp, LM Studio; design/31 section 3.3) that Settings lists and that goes `Offline`, never
//! gone, when the runtime stops. It carries no secret: the provider's id, the models as
//! `Discovered` claims, and a state. The report is an in-process call on the hosted service
//! (`keys::LocalKeys`); it was a bus call to accountd before.
//!
//! [`Reports`] is the seam: the key store in the daemon, a recorder in a unit test.

use crate::cloud::accountd::Boxed;
use crate::probe::Runtime;
use crate::probed::Standing;
use porter_capability::Claim;
use porter_core::AccountId;
use std::fmt::Debug;

/// Why a report was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFault {
    /// The store could not take it now: ask again at the next look.
    Unreachable,
    /// The store did not accept it (an unknown provider, a claim it does not accept).
    Rejected,
}

/// The key store, as inferd reports a runtime to it.
pub trait Reports: Debug + Send + Sync + 'static {
    /// Reports `runtime` in `standing` with its models as `claims`; the id of its account.
    fn report<'a>(
        &'a self,
        runtime: Runtime,
        claims: &'a [Claim],
        standing: Standing,
    ) -> Boxed<'a, Result<AccountId, ReportFault>>;
}

/// No key store: every report is `Unreachable`, so the daemon serves its probed runtimes on its
/// own.
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
