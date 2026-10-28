//! How a need meets an offer, sans I/O: what an app asks for ([`Need`] and the per-kind needs)
//! and the one function that says whether an [`porter_capability::Offer`] meets it
//! ([`matches`]), with the [`Shortfall`] that names what an offer lacks. Pure functions over
//! the capability vocabulary; no accounts, no I/O.

mod matching;
mod need;

pub use matching::{Match, Shortfall, matches};
pub use need::*;
