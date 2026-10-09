//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! `secrets` starts this test executable again as a fake child, by test path
//! (`secrets::oo7_child`).

mod client;
mod common;
mod google_levers;
mod secrets;
mod servers;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
