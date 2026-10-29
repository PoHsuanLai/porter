//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! A module that needs a family's feature carries it here or in its own `#![cfg]`.

mod acceptance;
#[cfg(feature = "agent_login")]
mod agent_login;
#[cfg(feature = "api_key")]
mod api_key;
mod common;
mod edit_service;
mod edit_servers;
mod generic;
#[cfg(feature = "oauth")]
mod google;
#[cfg(feature = "oauth")]
mod microsoft;
mod nextcloud;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
