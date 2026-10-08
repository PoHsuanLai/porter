//! Consent: grants per (app, account, kind, data class, use), decided purely.

mod decide;
mod grant;
mod prompt;

pub use decide::{Availability, Catalog, Verdict, availability, decide, decide_key};
pub use grant::{Decision, Grant, GrantKey, GrantScope, Usage};
pub use prompt::{AccountChoice, ConsentAnswer, ConsentAsk};
