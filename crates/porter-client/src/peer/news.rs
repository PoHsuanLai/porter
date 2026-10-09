//! Hearing from accountd that an account appeared, went or changed state.
//!
//! accountd sends its news only to the connections that have called it and may hear it (a porter
//! daemon hears an account come, go or change). A daemon joins by asking `Peer.Verdicts` once as
//! its own app, and again whenever accountd comes back on the bus (a new accountd has an empty
//! roster).

use super::{PeerAccounts, PeerError, chat_need};
use porter_core::AppId;
use porter_dbus::{ACCOUNTS_BUS, ACCOUNTS_PATH};
use zbus::export::futures_core::Stream;

/// The signals of accountd that can change the list of accounts or one's state.
const MEMBERS: [&str; 4] = [
    "AccountAdded",
    "AccountRemoved",
    "CapabilityChanged",
    "PropertiesChanged",
];

/// What the news said. More may be added: match with a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccountChange {
    /// An account came, went, or changed state or capabilities: read the accounts again.
    Accounts,
    /// accountd came back or went away (its name changed owner). The stream has joined the new
    /// accountd's roster; read the accounts again.
    Accountd,
}

/// The news of accounts, from now on. It ends (`next` answers `None`) when the connection does.
pub struct AccountNews {
    peer: PeerAccounts,
    app: AppId,
    signals: zbus::MessageStream,
    owners: zbus::fdo::NameOwnerChangedStream,
}

impl std::fmt::Debug for AccountNews {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountNews")
            .field("app", &self.app)
            .finish_non_exhaustive()
    }
}

async fn next_of<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

fn rule() -> zbus::Result<zbus::MatchRule<'static>> {
    Ok(zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(ACCOUNTS_BUS)?
        .path_namespace(ACCOUNTS_PATH)?
        .build())
}

impl AccountNews {
    /// Subscribes, then joins accountd's roster as `app`.
    pub(super) async fn join(peer: PeerAccounts, app: AppId) -> Result<Self, PeerError> {
        let signals = zbus::MessageStream::for_match_rule(rule()?, &peer.connection, None).await?;
        let owners = zbus::fdo::DBusProxy::new(&peer.connection)
            .await?
            .receive_name_owner_changed_with_args(&[(0, ACCOUNTS_BUS)])
            .await?;
        let news = Self {
            peer,
            app,
            signals,
            owners,
        };
        news.introduce().await;
        Ok(news)
    }

    /// Makes accountd know this connection: any call it identifies the app by is the
    /// introduction, and a failure (no accountd yet) is fine, the next owner change tries again.
    async fn introduce(&self) {
        let _ = self
            .peer
            .verdicts(
                &self.app,
                &chat_need(),
                porter_core::DataClass::Prompt,
                porter_core::consent::Usage::Interactive,
            )
            .await;
    }

    /// The next change, or `None` when the connection has ended.
    pub async fn next(&mut self) -> Option<AccountChange> {
        loop {
            tokio::select! {
                signal = next_of(&mut self.signals) => {
                    let message = signal?.ok()?;
                    let header = message.header();
                    let known = header
                        .member()
                        .is_some_and(|member| MEMBERS.contains(&member.as_str()));
                    if known {
                        return Some(AccountChange::Accounts);
                    }
                }
                owner = next_of(&mut self.owners) => {
                    owner?;
                    self.introduce().await;
                    return Some(AccountChange::Accountd);
                }
            }
        }
    }
}
