//! An agent program's sign-in state (agent-session ask P1, design/31 R7, R8): the launcher runs
//! the agent and reports whether the agent says it is signed in. That word is the only thing
//! porter holds for an `AgentLogin` account: no token, no key, no path to the agent's own login
//! files, which are the agent's and never read.

use crate::Accounts;
use porter_core::AccountId;
use porter_core::{AgentState, AuthKind};
use porter_service_core::audit::AuditEvent;
use porter_service_core::{Audited, Ports};

/// Why a report of an agent's state was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentFault {
    /// No account has this id.
    UnknownAccount,
    /// The account is not an agent that signs itself in; its state is porter's to set.
    NotAnAgent,
    /// The registry could not be saved; nothing changed.
    Unavailable,
}

impl<E: Ports> Accounts<'_, E> {
    /// See [`AccountsApi::set_agent_state`](crate::AccountsApi::set_agent_state).
    pub(crate) async fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> Result<bool, AgentFault> {
        let written = self
            .ctx
            .transact(|registry| {
                let Some(row) = registry.accounts.iter_mut().find(|a| a.id == *account) else {
                    return Ok((Vec::new(), Err(AgentFault::UnknownAccount)));
                };
                if row.auth != AuthKind::AgentLogin {
                    return Ok((Vec::new(), Err(AgentFault::NotAnAgent)));
                }
                let changed = std::mem::replace(&mut row.state, state.account_state())
                    != state.account_state();
                let lines = match changed {
                    true => vec![Audited::new(
                        None,
                        Some(account.clone()),
                        AuditEvent::AgentStateSet { state },
                    )],
                    false => Vec::new(),
                };
                Ok((lines, Ok(changed)))
            })
            .await
            .map_err(|_| AgentFault::Unavailable)?;
        written.value
    }
}
