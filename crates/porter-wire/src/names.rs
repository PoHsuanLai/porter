//! accountd's and the sheet host's bus names and object paths: the addresses an app, a shell or a
//! test needs to find the daemon or its settings module. Plain strings; the interfaces are
//! `porter-dbus`'s.

/// accountd's bus name.
pub const ACCOUNTS_BUS: &str = "org.quire.Accounts1";
/// accountd's root object (Manager, the add-account sheet).
pub const ACCOUNTS_PATH: &str = "/org/quire/Accounts1";
/// The desktop-wide Spaces (`org.quire.Spaces1`), served on accountd's bus name.
pub const SPACES_PATH: &str = "/org/quire/Spaces1";
/// The identities signed in on this machine (`org.quire.Identities1`), served on accountd's bus
/// name.
pub const IDENTITIES_PATH: &str = "/org/quire/Identities1";
/// The person's computers on their Tailscale network (`org.quire.Tailnet1`), served on
/// accountd's bus name.
pub const TAILNET_PATH: &str = "/org/quire/Tailnet1";
/// The sheet host's bus name (`org.quire.AccountsSheet1`, served by sill or a standalone host).
pub const SHEET_BUS: &str = "org.quire.AccountsSheet1";
/// The sheet host's object.
pub const SHEET_PATH: &str = "/org/quire/AccountsSheet1";
/// accountd's settings module (`org.quire.SettingsModule1`): the account, grant and client-id
/// rows the Accounts pane reads.
pub const ACCOUNTS_SETTINGS_PATH: &str = "/org/quire/Accounts1/settings";
