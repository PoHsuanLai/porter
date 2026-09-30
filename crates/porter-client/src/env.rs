//! What `Accounts::connect` may reach, injected by the app (CONVENTIONS §6: nothing ambient).

use std::path::PathBuf;

/// Where the latchkey socket lives (`$XDG_RUNTIME_DIR/porter/…` on Linux, a named pipe on
/// Windows), as the app resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketPath(pub PathBuf);

/// A daemon link to try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkChoice {
    /// accountd on the session bus.
    Dbus,
    /// accountd (or an app-hosted agent) on the latchkey socket.
    Socket(SocketPath),
}

/// The links to try, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEnv {
    /// The links, most preferred first.
    pub links: Vec<LinkChoice>,
}
