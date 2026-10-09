//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! A module that needs a family's feature carries it here or in its own `#![cfg]`.

mod acceptance;
#[cfg(feature = "agent_login")]
mod agent_login;
#[cfg(feature = "api_key")]
mod api_key;
mod common;
mod generic;
#[cfg(feature = "google")]
mod google;
#[cfg(feature = "microsoft")]
mod microsoft;
mod nextcloud;
#[cfg(feature = "openrouter")]
mod skeleton;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
