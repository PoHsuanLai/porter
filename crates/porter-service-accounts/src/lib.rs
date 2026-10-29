//! The accounts domain of the service (design/31 §4.1): add an account through the sheet, sign
//! it in again, ask for the access its provider asks only on use, look at what it can do again,
//! switch one of its kinds, and remove it; the agent programs that sign themselves in and the
//! local runtimes a probe reports are accounts too.
//!
//! [`AccountsApi`] is the domain's trait and [`Accounts`] is the view over a
//! [`porter_service_core::Ctx`] that implements it. What only this domain keeps between calls
//! (the roster of agent launchers, the person's own computers, the account ids an add is filing
//! secrets for) is [`AccountsState`]. `porter_service::AccountService` is the facade a host
//! holds over this domain and the grants'.

mod add;
mod add_flow;
mod add_store;
mod edit;
mod agent;
mod agent_login;
mod api;
mod launcher;
mod local;
mod manage;
mod own_machines;
mod race;
mod state;

pub use add_flow::SignInAs;
pub use add_store::AllowFor;
pub use agent::AgentFault;
pub use api::AccountsApi;
pub use edit::EditError;
pub use launcher::{Launchers, LoginEnd, NoLauncher, Roster, Waiting, program_of};
pub use local::{LocalFault, MAX_LOCAL_CLAIMS};
pub use own_machines::OwnMachines;
pub use state::AccountsState;

use porter_service_core::{Ctx, Ports};

/// The accounts domain over a context and its state: borrowed for the length of a call, so a
/// host builds one wherever it needs one.
pub struct Accounts<'c, E: Ports> {
    ctx: &'c Ctx<E>,
    state: &'c AccountsState,
}

impl<'c, E: Ports> Accounts<'c, E> {
    /// The accounts domain over `ctx`, with `state` as what it keeps between calls.
    pub fn over(ctx: &'c Ctx<E>, state: &'c AccountsState) -> Self {
        Self { ctx, state }
    }
}

impl<E: Ports> std::fmt::Debug for Accounts<'_, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Accounts").finish_non_exhaustive()
    }
}

impl<E: Ports> Clone for Accounts<'_, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E: Ports> Copy for Accounts<'_, E> {}
