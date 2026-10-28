//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! Modules that need a feature carry it here; the ones that start the daemons build only on
//! Linux, where the daemon's dev-dependency is (Cargo.toml).

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod common;
mod dbus_ai_accounts;
mod dbus_keys;
mod dbus_spaces;
mod dbus_tailnet;
mod end_to_end;
#[cfg(feature = "dbus")]
mod launcher;
mod process_credential;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
