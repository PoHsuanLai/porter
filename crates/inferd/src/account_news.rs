//! Hearing from accountd that a cloud AI account appeared, went or changed state, so inferd can
//! say `EnginesChanged` and a listener re-reads `Places`.
//!
//! accountd sends its news to the connections that have called it and may hear it (a porter
//! daemon hears an account come, go or change). inferd joins by asking `Peer.Verdicts` once for
//! its own app, and again whenever accountd comes back on the bus (a new accountd has an empty
//! roster). The news is a [`Notify`]: many changes in a moment are one wake-up.

use crate::cloud::accountd::llm_need;
use porter_core::{AppId, AppName, Isolation};
use porter_dbus::{ACCOUNTS_BUS, ACCOUNTS_PATH, PeerProxy, need_to_dbus};
use std::sync::Arc;
use tokio::sync::Notify;
use zbus::export::futures_core::Stream;

/// The app inferd names itself to accountd as.
const SELF_APP: &str = "org.quire.Inference";

/// The signals of accountd that can change the list of cloud accounts or one's state.
const MEMBERS: [&str; 4] = [
    "AccountAdded",
    "AccountRemoved",
    "CapabilityChanged",
    "PropertiesChanged",
];

async fn next_of<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

/// Joins accountd's roster: any call it identifies us by will do.
async fn join(connection: &zbus::Connection) {
    let Ok(peer) = PeerProxy::new(connection).await else {
        return;
    };
    let Ok(name) = AppName::parse(SELF_APP) else {
        return;
    };
    let app = AppId {
        name,
        isolation: Isolation::Unsandboxed,
    };
    let app = (
        app.name.as_str().to_owned(),
        crate::settings::slug_of(&app.isolation),
    );
    let _ = peer
        .verdicts(&app, &need_to_dbus(&llm_need()), "prompt", "interactive")
        .await;
}

/// Watches accountd until the connection ends, waking `news` on each change of accounts.
pub fn spawn(connection: zbus::Connection, news: Arc<Notify>) {
    tokio::spawn(async move {
        let Ok(rule) = news_rule() else {
            return;
        };
        let Ok(mut signals) = zbus::MessageStream::for_match_rule(rule, &connection, None).await
        else {
            return;
        };
        let Ok(bus) = zbus::fdo::DBusProxy::new(&connection).await else {
            return;
        };
        let Ok(mut owners) = bus
            .receive_name_owner_changed_with_args(&[(0, ACCOUNTS_BUS)])
            .await
        else {
            return;
        };
        join(&connection).await;
        loop {
            tokio::select! {
                signal = next_of(&mut signals) => {
                    let Some(Ok(message)) = signal else { return };
                    let header = message.header();
                    let known = header
                        .member()
                        .is_some_and(|member| MEMBERS.contains(&member.as_str()));
                    if known {
                        news.notify_one();
                    }
                }
                owner = next_of(&mut owners) => {
                    if owner.is_none() {
                        return;
                    }
                    join(&connection).await;
                    news.notify_one();
                }
            }
        }
    });
}

fn news_rule() -> zbus::Result<zbus::MatchRule<'static>> {
    Ok(zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(ACCOUNTS_BUS)?
        .path_namespace(ACCOUNTS_PATH)?
        .build())
}
