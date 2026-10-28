//! The grants domain of the service (design/31 §4.1): which accounts an app may use, the chooser
//! and consent sheet that give it one, the end of a grant (revoked, spent by its first use, or
//! over with its session), and the relays a grant opens to its account's servers.
//!
//! [`GrantsApi`] is the domain's trait and [`Grants`] is the view over a
//! [`porter_service_core::Ctx`] that implements it. A chooser whose person asks to add an
//! account goes through the accounts domain, which the view holds beside the context.

mod api;
mod audience;
mod grants;
mod llm;
mod relay;

pub use api::GrantsApi;

use porter_service_accounts::Accounts;
use porter_service_core::{Ctx, Ports};

/// The grants domain over a context, and the accounts domain it asks to add an account.
pub struct Grants<'c, E: Ports> {
    ctx: &'c Ctx<E>,
    accounts: Accounts<'c, E>,
}

impl<'c, E: Ports> Grants<'c, E> {
    /// The grants domain over `ctx`; `accounts` adds the account a chooser's "Add Account…" asks
    /// for.
    pub fn over(ctx: &'c Ctx<E>, accounts: Accounts<'c, E>) -> Self {
        Self { ctx, accounts }
    }
}

impl<E: Ports> std::fmt::Debug for Grants<'_, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grants").finish_non_exhaustive()
    }
}
