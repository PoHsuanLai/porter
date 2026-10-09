//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! Modules that need a feature carry it here. `connect` and `socket_accounts` are separate
//! targets (Cargo.toml): each starts its own test executable again as a fake child by test name.

mod accountd_requests;
#[cfg(any(feature = "dbus", feature = "socket"))]
mod common;
mod dbus_accounts;
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
