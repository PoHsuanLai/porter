//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! Modules that need a feature carry it here; the ones that start the daemons build only on
//! Linux, where the daemons' dev-dependencies are (Cargo.toml). `connect` starts this executable
//! again as a fake child, by test path.

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod common;
#[cfg(all(feature = "dbus", feature = "infer"))]
mod connect;
mod dbus_guests;
mod dbus_keys;
mod dbus_open;
mod dbus_served;
mod dbus_spaces;
mod dbus_tailnet;
mod end_to_end;
#[cfg(feature = "engines")]
mod engines;
mod in_process_session;
#[cfg(feature = "dbus")]
mod launcher;
mod prepare;
mod process_credential;
mod session_shape;
#[cfg(feature = "lending")]
mod tailnet_lending;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
