//! The client crates against the real accountd: `porter-client` over `porter-client-dbus`'s `Bus`,
//! on a private bus. They live here, not in the client crate, because a client crate must build
//! and be semver-checked without building a daemon.

mod common;
mod dbus_accounts;
mod dbus_ai_accounts;
mod dbus_keys;
mod dbus_spaces;
mod dbus_tailnet;
mod launcher;
mod process_credential;
