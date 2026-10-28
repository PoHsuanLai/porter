//! The launcher side of an agent's sign-in, as values: what a launcher answered ([`LoginEnd`]),
//! the seam accountd serves over the bus ([`Launchers`]), which programs have a launcher
//! ([`Roster`]), and the program an agent account or provider file runs.

use porter_capability::AgentProgram;
use porter_capability::{Capability, Offer, Subject};
use porter_core::AccountId;
use porter_core::{Account, LoginFault, LoginOutcome};
use porter_sheet::SignInFault;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// How a request to a launcher ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoginEnd {
    /// The launcher reported this.
    Reported(LoginOutcome),
    /// The launcher did not report before accountd's bound ran out.
    Expired,
    /// The launcher's connection left the bus with the request pending.
    LauncherGone,
}

impl LoginEnd {
    /// What the sheet says of an end that was not a success.
    pub fn fault(self) -> SignInFault {
        match self {
            LoginEnd::Reported(LoginOutcome::Ready) => SignInFault::Unreadable,
            LoginEnd::Reported(LoginOutcome::Cancelled) => SignInFault::Cancelled,
            LoginEnd::Reported(LoginOutcome::Failed(fault)) => match fault {
                LoginFault::Refused => SignInFault::Refused,
                LoginFault::Unreachable => SignInFault::Unreachable,
                LoginFault::NotInstalled => SignInFault::NotInstalled,
                LoginFault::TimedOut => SignInFault::TimedOut,
                LoginFault::Other => SignInFault::Unreadable,
                // a variant a newer porter adds: the sign-in ends as unreadable
                _ => SignInFault::Unreadable,
            },
            // a variant a newer porter adds: the sign-in ends as unreadable
            LoginEnd::Reported(_) => SignInFault::Unreadable,
            LoginEnd::Expired => SignInFault::Expired,
            LoginEnd::LauncherGone => SignInFault::NoLauncher,
        }
    }
}

/// No launcher is registered for the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoLauncher;

/// Which agent programs have a launcher right now: what the add sheet asks before an agent
/// account is made (accountd reads its registrations).
#[derive(Clone)]
pub struct Roster(Arc<dyn Fn(&AgentProgram) -> bool + Send + Sync>);

impl Roster {
    /// A roster answering by `registered`.
    pub fn new(registered: impl Fn(&AgentProgram) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(registered))
    }

    pub(crate) fn has(&self, program: &AgentProgram) -> bool {
        (self.0)(program)
    }
}

impl std::fmt::Debug for Roster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Roster")
    }
}

/// The end of a request, waited for. Dropping it does not withdraw the request: it stays pending
/// until the launcher answers, the bound runs out, or the launcher leaves.
pub type Waiting = Pin<Box<dyn Future<Output = LoginEnd> + Send>>;

/// The launchers registered for agent programs: who can be asked to sign an agent in or out.
pub trait Launchers: Send + Sync {
    /// Asks the launcher of `program` to sign `account` in. The request has been sent when this
    /// returns; the answer is `Waiting`.
    fn ask_login(
        &self,
        account: &AccountId,
        program: &AgentProgram,
    ) -> impl Future<Output = Result<Waiting, NoLauncher>> + Send;

    /// Asks the launcher of `program` to sign `account` out.
    fn ask_logout(
        &self,
        account: &AccountId,
        program: &AgentProgram,
    ) -> impl Future<Output = Result<Waiting, NoLauncher>> + Send;
}

/// The program an agent account runs: the one its claims are about. None for an account that
/// names none.
pub fn program_of(account: &Account) -> Option<AgentProgram> {
    account
        .capabilities
        .iter()
        .find_map(|claim| match (&claim.subject, &claim.offer) {
            (Subject::Agent(program), Offer::Present(Capability::Agent(_))) => {
                Some(program.clone())
            }
            _ => None,
        })
}

/// The program a provider file for an agent runs: the one its agent capability names.
pub(crate) fn program_of_spec(spec: &porter_provider::ProviderSpec) -> Option<AgentProgram> {
    spec.capabilities
        .iter()
        .find_map(|row| match &row.capability {
            Capability::Agent(agent) => Some(agent.program.clone()),
            _ => None,
        })
}

#[cfg(test)]
mod tests;
