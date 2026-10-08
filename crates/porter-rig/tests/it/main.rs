//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! `secrets` is a separate target (Cargo.toml): it starts this test executable again as a fake
//! child by test name.

mod client;
mod common;
mod google_levers;
mod servers;
