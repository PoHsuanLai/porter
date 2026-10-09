//! Hearing from accountd that a cloud AI account appeared, went or changed state, so inferd can
//! say `EnginesChanged` and a listener re-reads `Places`.
//!
//! accountd sends its news to the connections that have called it and may hear it (a porter
//! daemon hears an account come, go or change). inferd joins as its own app through
//! porter-client's [`PeerAccounts::news`], which asks `Peer.Verdicts` once and again whenever
//! accountd comes back on the bus (a new accountd has an empty roster). The news is a [`Notify`]:
//! many changes in a moment are one wake-up.

use porter_client::peer::PeerAccounts;
use porter_core::{AppId, AppName, Isolation};
use std::sync::Arc;
use tokio::sync::Notify;

/// The app inferd names itself to accountd as.
const SELF_APP: &str = "org.quire.Inference";

/// Watches accountd until the connection ends, waking `news` on each change of accounts.
pub fn spawn(connection: zbus::Connection, news: Arc<Notify>) {
    tokio::spawn(async move {
        let Ok(name) = AppName::parse(SELF_APP) else {
            return;
        };
        let app = AppId {
            name,
            isolation: Isolation::Unsandboxed,
        };
        let Ok(mut changes) = PeerAccounts::over(&connection).news(&app).await else {
            return;
        };
        while changes.next().await.is_some() {
            news.notify_one();
        }
    });
}
