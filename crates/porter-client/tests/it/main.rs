//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! Modules that need a feature carry it here. `socket_accounts` is a separate target
//! (Cargo.toml): it builds without porter-infer. `connect` starts this executable again as a
//! fake child, by test path.

mod accountd_requests;
#[cfg(any(feature = "dbus", feature = "socket"))]
mod common;
#[cfg(all(feature = "dbus", feature = "infer"))]
mod connect;
mod dbus_accounts;
mod dbus_guests;
mod dbus_open;
mod dbus_peer;
mod dbus_served;
mod dbus_sheets;
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
mod socket;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
