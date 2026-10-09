//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).

mod common;
mod contract;
mod fixtures;
mod graph;
mod linked;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
