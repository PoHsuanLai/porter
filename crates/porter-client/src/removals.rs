//! `AccountRemoved`: hearing from accountd that an account is gone, so an app that keeps
//! something of it (a daemon with the account's mirrors and journals) can forget it.
//!
//! The signal is accountd's, unicast to the apps that hold a grant for the account; the proxy
//! matches the sender against the owner of `org.quire.Accounts1`, so a signal another connection
//! sends is not heard. accountd tells only the connections that have called it, so the stream
//! calls it once at the start and again whenever accountd gets a new owner on the bus
//! (`Grants.List`, the caller's own grants): without that a restarted accountd would not know
//! there is an app to tell.

use crate::error::TransportError;
use crate::transport::bus_error;
use porter_dbus::{
    ACCOUNTS_BUS, ACCOUNTS_PATH, AccountRemovedStream, BusConnection, GrantsProxy, ManagerProxy,
};
use zbus::export::futures_core::Stream;
use zbus::fdo::DBusProxy;

/// An account accountd has removed, as the object path it was served at.
///
/// The path's last segment is a lossy spelling of the account id (it is a legal object-path
/// segment), good as a directory name and not to be turned back into an id.
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

/// The accounts accountd removes, from now on. Dropping it stops the watch for a new accountd.
#[derive(Debug)]
pub struct Removals {
    removals: AccountRemovedStream,
    introducer: tokio::task::JoinHandle<()>,
}

impl Removals {
    /// Subscribes, then introduces this connection to accountd, now and at each new owner.
    pub(crate) async fn watch(connection: &BusConnection) -> Result<Self, TransportError> {
        let removals = ManagerProxy::new(connection)
            .await
            .map_err(|e| bus_error(&e))?
            .receive_account_removed()
            .await
            .map_err(|e| bus_error(&e))?;
        let mut owners = DBusProxy::new(connection)
            .await
            .map_err(|e| bus_error(&e))?
            .receive_name_owner_changed_with_args(&[(0, ACCOUNTS_BUS)])
            .await
            .map_err(|e| bus_error(&e))?;
        let announce = connection.clone();
        let introducer = tokio::spawn(async move {
            introduce(&announce).await;
            while let Some(signal) =
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut owners).poll_next(cx)).await
            {
                if signal.args().is_ok_and(|args| args.new_owner().is_some()) {
                    introduce(&announce).await;
                }
            }
        });
        Ok(Self {
            removals,
            introducer,
        })
    }

    /// The next removed account, or `None` when the connection has ended. A signal that is not
    /// well formed, or names no account path, is skipped.
    pub async fn next(&mut self) -> Option<RemovedAccount> {
        loop {
            let signal =
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut self.removals).poll_next(cx))
                    .await?;
            let Ok(args) = signal.args() else { continue };
            if let Some(account) = RemovedAccount::of_object_path(args.account().as_str()) {
                return Some(account);
            }
        }
    }
}

impl Drop for Removals {
    fn drop(&mut self) {
        self.introducer.abort();
    }
}

/// Makes accountd know this connection: the call itself is the introduction, and a failure (no
/// accountd yet) is fine, the next owner change tries again.
async fn introduce(connection: &BusConnection) {
    if let Ok(grants) = GrantsProxy::new(connection).await {
        let _ = grants.list().await;
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
