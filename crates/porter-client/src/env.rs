//! What `Accounts::connect` may reach, injected by the app (CONVENTIONS §6: nothing ambient).

/// A daemon link to try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkChoice {
    /// accountd on the session bus.
    Dbus,
}

/// The links to try, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEnv {
    /// The links, most preferred first.
    pub links: Vec<LinkChoice>,
}
