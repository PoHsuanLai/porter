//! The AI accounts as inferd uses them: accountd keeps the keys, the accounts and the permits
//! (`org.quire.Accounts1.Broker`), and inferd, the one daemon that sends a key to an AI company,
//! asks it through `porter-client`'s [`AiAccounts`]. [`AccountdBroker`] is both seams the rest of
//! the daemon asks through: [`Accountd`] (which accounts an app holds a permit on, and the key
//! behind a permit) and [`Reports`] (a probed runtime becoming an account).
//!
//! A key is read for one request and dropped, as a [`SecretText`] that shows nothing of itself;
//! nothing here caches one. [`follow`] keeps the list of accounts the spend rows are listed over,
//! and wakes the places listeners when accountd says something changed.

use crate::cloud::accountd::{AccountVerdict, Accountd, AccountdFault, Boxed};
use crate::cloud::own::OwnServer;
use crate::probe::Runtime;
use crate::probed::Standing;
use crate::report::{ReportFault, Reports};
use porter_capability::Claim;
use porter_client::{AiAccounts, AiAccountsError, LinkError};
use porter_client_dbus::Bus;
use porter_core::consent::Usage;
use porter_core::{AccountId, AccountState, AiAccountRow, AppId, DataClass, GrantId};
use porter_core::{ProviderId, SecretText};
use porter_dbus::BusStream;
use porter_wire::Refusal;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;

/// accountd's AI accounts, asked over the bus.
#[derive(Debug, Clone)]
pub struct AccountdBroker {
    accounts: AiAccounts<Bus>,
}

impl AccountdBroker {
    /// The accounts of accountd, asked through `accounts`.
    pub fn new(accounts: AiAccounts<Bus>) -> Self {
        Self { accounts }
    }
}

/// What a failed call is to the one who asked: a refusal is final, an accountd that is not there
/// or cannot answer now is "ask again at the next look".
fn fault_of(error: AiAccountsError) -> AccountdFault {
    match error {
        AiAccountsError::Refused(refusal) => AccountdFault::from(refusal),
        AiAccountsError::Link(LinkError::Unreachable) => AccountdFault::Unreachable,
        AiAccountsError::Link(LinkError::Malformed(_)) => AccountdFault::Unreadable,
        // A caller accountd does not know as the broker, or an argument it refused: final.
        _ => AccountdFault::Refused,
    }
}

impl Accountd for AccountdBroker {
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>> {
        Box::pin(async move {
            let rows = self
                .accounts
                .verdicts(app, class, usage)
                .await
                .map_err(fault_of)?;
            Ok(rows
                .into_iter()
                .map(|row| AccountVerdict {
                    account: row.account,
                    provider: Some(row.provider),
                    label: Some(row.label),
                    provider_label: row.provider_label,
                    state: Some(row.state),
                    verdict: row.verdict,
                })
                .collect())
        })
    }

    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async move { self.accounts.key(grant).await.map_err(fault_of) })
    }

    fn servers(&self) -> Boxed<'_, Vec<OwnServer>> {
        Box::pin(async move {
            // A look that fails is no servers: the next look asks again.
            let rows = self.accounts.servers().await.unwrap_or_default();
            rows.into_iter()
                .filter_map(|row| {
                    OwnServer::of(row.account, row.label, std::slice::from_ref(&row.endpoint))
                })
                .collect()
        })
    }

    fn server_key<'a>(
        &'a self,
        account: &'a AccountId,
    ) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async move { self.accounts.server_key(account).await.map_err(fault_of) })
    }
}

impl Reports for AccountdBroker {
    fn report<'a>(
        &'a self,
        runtime: Runtime,
        claims: &'a [Claim],
        standing: Standing,
    ) -> Boxed<'a, Result<AccountId, ReportFault>> {
        Box::pin(async move {
            let provider =
                ProviderId::parse(runtime.provider()).map_err(|_| ReportFault::Rejected)?;
            let state = match standing {
                Standing::Online => AccountState::Ok,
                Standing::Offline => AccountState::Offline,
            };
            self.accounts
                .report_local(&provider, claims, state)
                .await
                .map_err(|error| match error {
                    AiAccountsError::Link(LinkError::Unreachable)
                    | AiAccountsError::Refused(Refusal::Unavailable) => ReportFault::Unreachable,
                    _ => ReportFault::Rejected,
                })
        })
    }
}

/// The AI accounts as the spend rows list them, kept current by [`follow`].
#[derive(Debug, Clone, Default)]
pub struct AccountList(Arc<Mutex<Vec<AiAccountRow>>>);

impl AccountList {
    /// The accounts now.
    pub fn now(&self) -> Vec<AiAccountRow> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the list with `rows`.
    pub fn replace(&self, rows: Vec<AiAccountRow>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = rows;
    }
}

/// How long to wait before asking an accountd that was not there again.
const RETRY: Duration = Duration::from_secs(2);

/// Keeps `list` current and wakes `news` each time accountd says an AI account, a permit or a
/// switch changed (so a listener reads `Places` again). It subscribes before it looks, asks
/// again when accountd comes back after it went, and ends with the runtime. `first` runs once,
/// after the first look that worked: accountd is up then, and has copied what it found of
/// inferd's old store.
pub fn follow(
    accounts: AiAccounts<Bus>,
    list: AccountList,
    news: Arc<Notify>,
    first: impl FnOnce() + Send + 'static,
) {
    tokio::spawn(async move {
        let mut first = Some(first);
        loop {
            // Subscribed before the first look, so a change between the two is not missed.
            let Ok(mut changes) = accounts.changes().await else {
                tokio::time::sleep(RETRY).await;
                continue;
            };
            loop {
                match accounts.accounts().await {
                    Ok(rows) => {
                        list.replace(rows);
                        if let Some(first) = first.take() {
                            first();
                        }
                    }
                    Err(error) => eprintln!("inferd: the AI accounts could not be listed: {error}"),
                }
                news.notify_one();
                let next =
                    std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)).await;
                if next.is_none() {
                    break;
                }
            }
            tokio::time::sleep(RETRY).await;
        }
    });
}
