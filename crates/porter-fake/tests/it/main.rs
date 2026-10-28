//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).

mod add_flow;
mod fakes;
mod grant_kind;
mod llm_keys;
mod local;
mod persist;
mod refresh;
mod service;
mod session_scope;
mod space_owner;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
