//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).

mod acceptance;
mod account_objects;
mod add;
mod agent_launcher;
mod agent_login;
mod binary;
mod bus_sheets;
mod common;
mod daemon;
mod dist;
mod file_keys;
mod google;
mod install;
mod peer;
mod process_credential;
mod relay;
mod report_local;
mod resolve_key;
mod roles;
mod scenario_fixes;
mod session_scope;
mod settings;
mod shell;
mod signals;
mod spaces;
mod start;
mod tailnet;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
