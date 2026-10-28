//! `org.quire.AccountsSheet1` at `/org/quire/AccountsSheet1`: the sheet host's side (sill on our
//! desktop, a standalone host elsewhere). accountd calls `Open`, `Update` and `Close` with a
//! `SheetView` as JSON; the host answers with `Input` signals carrying a `SheetInput` as JSON.
//!
//! Only accountd's connection may call `Open`, `Update` and `Close`, and only the connection that
//! opened a handle may send its `Input`: the host checks the sender of each call, accountd the
//! sender of each signal. An `Input` may carry secret text (a typed password) from the sheet
//! into accountd, the one place secret text flows into the daemon; nothing here flows outward.

use porter_dbus_macros::bus_interface;

/// The interface, written once. `#[bus_interface]` makes [`AccountsSheetProxy`] for its callers from this
/// trait, [`AccountsSheetServer`] for the daemon to implement and [`AccountsSheetServed`] to register.
#[bus_interface(
    interface = "org.quire.AccountsSheet1",
    default_service = "org.quire.AccountsSheet1",
    default_path = "/org/quire/AccountsSheet1"
)]
pub trait AccountsSheet {
    /// Shows a sheet for `handle`, attached to `parent_window` (empty: none), drawing `view`.
    fn open(&self, handle: &str, parent_window: &str, view: &str) -> zbus::Result<()>;
    /// Replaces what the sheet of `handle` shows.
    fn update(&self, handle: &str, view: &str) -> zbus::Result<()>;
    /// Takes the sheet of `handle` down.
    fn close(&self, handle: &str) -> zbus::Result<()>;
    /// What the person did on the sheet of `handle`.
    #[zbus(signal)]
    fn input(&self, handle: &str, input: &str) -> zbus::Result<()>;
}
