//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).

mod acceptance;
mod account_objects;
mod binary;
mod broker;
mod bus_sheets;
mod client;
mod common;
mod daemon;
mod dist;
mod file_keys;
mod google;
mod grants;
mod identities;
mod install;
mod keys;
mod launcher_credential;
mod launcher_login;
mod launcher_register;
mod launcher_rig;
mod launcher_session;
mod old_keys;
mod roles;
mod scenario_fixes;
mod settings;
mod shell;
mod signals;
mod spaces;
mod start;
mod tailnet;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
