//! Hearing from accountd that an identity is gone, so an app that keeps something of it (a copy of
//! its sign-in, mirrors and journals) can forget it.
//!
//! This reads `Identities1`'s `IdentityRemoved` signal ([`crate::Identities::watch_removals`]) and
//! gives each removal as the account's object-path segment, the spelling that names an app's
//! directories. The signal is accountd's, and only a connection accountd knows is told.

use crate::error::TransportError;
use crate::identities::{Identities, IdentitiesError, IdentityRemovals};
use porter_core::{AccountId, object_segment};
use porter_dbus::{ACCOUNTS_PATH, BusConnection};
use zbus::export::futures_core::Stream;

/// An identity accountd has removed, as the object-path segment its account was served at.
///
/// The segment is a lossy spelling of the id (it is a legal object-path segment), good as a
/// directory name and not to be turned back into an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedAccount {
    segment: String,
}

impl RemovedAccount {
    /// The account the object path `/org/quire/Accounts1/account/<segment>` names; `None` for any
    /// other path.
    pub fn of_object_path(path: &str) -> Option<Self> {
        let prefix = format!("{ACCOUNTS_PATH}/account/");
        let segment = path.strip_prefix(&prefix)?;
        (!segment.is_empty() && !segment.contains('/')).then(|| Self {
            segment: segment.to_owned(),
        })
    }

    /// The account's segment: what names its directories (`a1`, `67e55044_10b1_x`).
    pub fn segment(&self) -> &str {
        &self.segment
    }
}

/// The identities accountd removes, from now on.
#[derive(Debug)]
pub struct Removals {
    removals: IdentityRemovals,
}

fn transport(error: IdentitiesError) -> TransportError {
    match error {
        IdentitiesError::Unreachable => TransportError::Unreachable,
        IdentitiesError::Denied(why) => TransportError::Denied(why),
        other => TransportError::Malformed(other.to_string()),
    }
}

impl Removals {
    /// Subscribes to `Identities1`'s removals over `connection`.
    pub(crate) async fn watch(
        connection: &BusConnection,
        to: &crate::Destination,
    ) -> Result<Self, TransportError> {
        let removals = Identities::connect_at(connection, to)
            .await
            .map_err(transport)?
            .watch_removals()
            .await
            .map_err(transport)?;
        Ok(Self { removals })
    }

    /// The next removed identity, or `None` when the connection has ended. A signal that is not
    /// well formed is skipped.
    pub async fn next(&mut self) -> Option<RemovedAccount> {
        loop {
            let signal =
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut self.removals).poll_next(cx))
                    .await?;
            let Ok(id) = signal else { continue };
            // An identity is the id of the account it is.
            let Ok(account) = AccountId::parse(id.as_str()) else {
                continue;
            };
            return Some(RemovedAccount {
                segment: object_segment(&account),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_the_last_segment_of_its_object_path_and_nothing_else_is_one() {
        let path = "/org/quire/Accounts1/account/abc_1";
        assert_eq!(
            RemovedAccount::of_object_path(path).map(|a| a.segment().to_owned()),
            Some("abc_1".to_owned())
        );
        for other in [
            "/org/quire/Accounts1/account/",
            "/org/quire/Accounts1/account/a/b",
            "/other/abc",
        ] {
            assert_eq!(RemovedAccount::of_object_path(other), None, "{other}");
        }
    }
}
