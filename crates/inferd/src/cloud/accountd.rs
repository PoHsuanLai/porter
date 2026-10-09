//! What inferd asks accountd for a hosted model: which accounts an app holds a grant on
//! (`Peer.Verdicts`) and, for the one it will call, the account's API key (`Peer.ResolveKey`).
//!
//! The key arrives on a sealed memfd, never as text on the bus. It is read once into a
//! [`SecretText`] (whose `Debug` shows nothing) that the caller drops after building one request.
//! [`Accountd`] is the seam: the bus peer in the daemon, a scripted fake in the unit tests. The
//! bus side is porter-client's [`PeerAccounts`], which also reads the answer; this module only
//! says what inferd does about each way it can fail ([`AccountdFault`]).

use porter_client::peer::PeerError;
pub(crate) use porter_client::peer::chat_need as llm_need;
pub use porter_client::peer::{AccountVerdict, PeerAccounts};
use porter_core::consent::Usage;
use porter_core::{AppId, DataClass, GrantId, SecretText};
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

/// A boxed future, so the seam can be a trait object.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Why accountd did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountdFault {
    /// Not on the bus, or the call failed.
    Unreachable,
    /// accountd refused this caller or this grant.
    Refused,
    /// The answer was not what the interface promises (a key not on a sealed file).
    Unreadable,
}

impl From<&PeerError> for AccountdFault {
    /// What inferd does about it: a refusal is final, an unreadable key is its own, and any other
    /// failure is "ask again at the next look".
    fn from(error: &PeerError) -> Self {
        match error {
            PeerError::Denied(_) => AccountdFault::Refused,
            PeerError::Unreadable => AccountdFault::Unreadable,
            _ => AccountdFault::Unreachable,
        }
    }
}

impl From<PeerError> for AccountdFault {
    fn from(error: PeerError) -> Self {
        AccountdFault::from(&error)
    }
}

/// accountd, as inferd uses it.
pub trait Accountd: Debug + Send + Sync + 'static {
    /// The grants `app` holds, for data of `class` used as `usage`, on every account that serves a language need.
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>>;

    /// The API key behind `grant`.
    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>>;
}

/// accountd over the session bus: `org.quire.Accounts1.Peer`, called by a porter daemon.
#[derive(Debug, Clone)]
pub struct PeerAccountd {
    peer: PeerAccounts,
}

impl PeerAccountd {
    /// Asks accountd over `connection`.
    pub fn new(connection: zbus::Connection) -> Self {
        Self {
            peer: PeerAccounts::over(&connection),
        }
    }
}

impl Accountd for PeerAccountd {
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>> {
        Box::pin(async move { Ok(self.peer.verdicts(app, &llm_need(), class, usage).await?) })
    }

    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async move { Ok(self.peer.resolve_key(grant).await?) })
    }
}

#[cfg(test)]
mod tests;
