//! `org.quire.Accounts1.Account`: one object per account, at `porter_dbus::accounts::account_path`.
//! `Reauthenticate` and the properties (`Id`, `Provider`, `Label`, `State`, `ReauthReason`,
//! `Capabilities`) are served; every one answers only the shell (`SheetHost`) and Settings, the two
//! roles that read an account. No app holds a grant for an account any more: an app learns of an
//! identity through `Identities1`.
//!
//! For any other caller an account does not exist (`UnknownObject`): there is no bulk
//! enumeration (design/31 §4.4).

use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use crate::state::{Core, Standing, window};
use porter_callers::{Caller, CallerRole};
use porter_core::{Account, AccountState};
use porter_dbus::accounts::sheet::SheetKind;
use porter_dbus::accounts::{ACCOUNTS_PATH, account_path};
use porter_dbus::accounts::{AccountServed, AccountServer};
use porter_dbus::{Call, Details, to_vardict};
use porter_ids::object_segment;
use porter_ids::word::word as word_of;
use porter_wire::AccountsRequest;
use std::sync::Arc;
use zbus::ObjectServer;
use zbus::fdo;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;

/// One account's object.
#[derive(Debug)]
pub(crate) struct AccountObject<H, C>(Arc<Core<H, C>>);

impl<H, C> AccountObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// The closed word for why `account` needs signing in again, or empty.
pub(crate) fn reauth_reason_word(account: &Account, now: porter_ids::UnixSeconds) -> &'static str {
    match account.state {
        AccountState::NeedsReauth => account
            .restriction
            .reauth_reason(now)
            .map_or("", |reason| word_of(&reason)),
        _ => "",
    }
}

/// What a property read needs: the account at this path, for the shell or Settings.
async fn visible<H: Host, C: Callers>(
    core: &Core<H, C>,
    header: Option<&Header<'_>>,
) -> fdo::Result<Account> {
    let header = header.ok_or_else(|| fdo::Error::AccessDenied("no caller".into()))?;
    let caller = core
        .identify(header, Standing::Any)
        .await
        .map_err(|_| fdo::Error::AccessDenied("accountd does not know this caller".into()))?;
    let registry = core.host.registry();
    let path = header.path().map(|p| p.as_str()).unwrap_or_default();
    account_at(&registry.accounts, path)
        .filter(|_| sees(&caller))
        .cloned()
        .ok_or_else(|| fdo::Error::UnknownObject("no such account for this caller".into()))
}

/// The account an object path stands for, among `accounts`.
fn account_at<'a>(accounts: &'a [Account], path: &str) -> Option<&'a Account> {
    let segment = path.strip_prefix(&format!("{ACCOUNTS_PATH}/account/"))?;
    accounts
        .iter()
        .find(|account| object_segment(account.id.as_str()) == segment)
}

/// Whether `caller` may see accounts: only the shell, which tells the person when an account
/// must be signed in again, and Settings, which lists them.
fn sees(caller: &Caller) -> bool {
    matches!(caller.role, CallerRole::SheetHost | CallerRole::Settings)
}

impl<H: Host, C: Callers> AccountServer for AccountObject<H, C> {
    type Error = RefusedError;

    async fn id(&self, header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(visible(&self.0, header.as_ref()).await?.id.to_string())
    }

    async fn provider(&self, header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(visible(&self.0, header.as_ref())
            .await?
            .provider
            .to_string())
    }

    async fn label(&self, header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(visible(&self.0, header.as_ref()).await?.label.0)
    }

    async fn state(&self, header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(porter_ids::word::word(&visible(&self.0, header.as_ref()).await?.state).to_owned())
    }

    /// Why the account has to be signed in again, when porter knows: a word of
    /// `ReauthReason`, else empty.
    async fn reauth_reason(&self, header: Option<Header<'_>>) -> fdo::Result<String> {
        let account = visible(&self.0, header.as_ref()).await?;
        let now = self.0.clock.now();
        Ok(reauth_reason_word(&account, now).to_owned())
    }

    /// The account's effective capabilities: each kind's slug and the claim's fields by name.
    async fn capabilities(
        &self,
        header: Option<Header<'_>>,
    ) -> fdo::Result<Vec<(String, Details)>> {
        let account = visible(&self.0, header.as_ref()).await?;
        Ok(account
            .capabilities
            .iter()
            .filter_map(|claim| {
                let slug = serde_json::to_value(claim.offer.kind()).ok()?;
                let fields = match serde_json::to_value(claim).ok()? {
                    serde_json::Value::Object(fields) => fields,
                    _ => return None,
                };
                Some((slug.as_str()?.to_owned(), to_vardict(&fields)))
            })
            .collect())
    }

    async fn reauthenticate(
        &self,
        Call { header, .. }: Call<'_>,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let caller = self.0.identify(&header, Standing::Acting).await?;
        let registry = self.0.host.registry();
        let path = header.path().map(|p| p.as_str()).unwrap_or_default();
        let account = account_at(&registry.accounts, path)
            .filter(|_| sees(&caller))
            .map(|account| account.id.clone())
            .ok_or_else(|| RefusedError::unknown_object("no such account for this caller"))?;
        let request = AccountsRequest::Reauthenticate {
            account,
            window: window(&parent_window),
        };
        self.0
            .sheet(&header, SheetKind::Reauthenticate, &options, request)
            .await
    }
}

/// Registers an object for every account the registry holds that has none yet.
pub(crate) async fn publish_accounts<H: Host, C: Callers>(
    server: &ObjectServer,
    core: &Arc<Core<H, C>>,
) -> zbus::Result<()> {
    for account in core.host.registry().accounts {
        server
            .at(
                account_path(&account.id),
                AccountServed(AccountObject(Arc::clone(core))),
            )
            .await?;
    }
    Ok(())
}
