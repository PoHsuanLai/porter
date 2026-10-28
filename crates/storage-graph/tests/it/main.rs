//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).

mod common;
mod contract;
mod drive_write;
mod fixtures;
mod graph;
mod linked;
mod whole_drive;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
