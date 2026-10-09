//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).

mod add_flow;
mod grant_kind;
mod local;
mod persist;
mod session_scope;
mod space_owner;
mod sync_grant;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
