//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! Modules that need a feature carry it here; the ones that start the daemon build only on Linux,
//! where the daemon's dev-dependencies are (Cargo.toml). `connect` starts this executable again as
//! a fake child, by test path.

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod common;
#[cfg(feature = "dbus")]
mod connect;
mod dbus_guests;
mod dbus_open;
mod dbus_served;
#[cfg(feature = "engines")]
mod engines;
mod in_process_session;
mod prepare;
mod session_shape;
#[cfg(feature = "lending")]
mod tailnet_lending;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
