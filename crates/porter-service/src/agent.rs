//! An agent program's sign-in state (agent-session ask P1, design/31 R7, R8): the launcher runs
//! the agent and reports whether the agent says it is signed in. That word is the only thing
//! porter holds for an `AgentLogin` account: no token, no key, no path to the agent's own login
//! files, which are the agent's and never read.

use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::audit::AuditEvent;
use porter_core::{AccountId, AgentState, AuthKind};
use porter_provider::Provider;
use porter_secrets::Secrets;

/// Why a report of an agent's state was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentFault {
    /// No account has this id.
    UnknownAccount,
    /// The account is not an agent that signs itself in; its state is porter's to set.
    NotAnAgent,
    /// The registry could not be saved; the change stays in memory.
    Unavailable,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Records what the agent says of its own sign-in. Returns whether the account's state
    /// changed (a repeat of the same word changes nothing and writes nothing).
    pub async fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> Result<bool, AgentFault> {
        let changed = {
            let mut registry = self.lock();
            let row = registry
                .accounts
                .iter_mut()
                .find(|a| a.id == *account)
                .ok_or(AgentFault::UnknownAccount)?;
            if row.auth != AuthKind::AgentLogin {
                return Err(AgentFault::NotAnAgent);
            }
            std::mem::replace(&mut row.state, state.account_state()) != state.account_state()
        };
        if changed {
            self.persist().await.map_err(|_| AgentFault::Unavailable)?;
            self.note(
                None,
                Some(account.clone()),
                AuditEvent::AgentStateSet { state },
            );
        }
        Ok(changed)
    }
}
