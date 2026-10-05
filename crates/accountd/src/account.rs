//! `org.quire.Accounts1.Account`: one object per account, at `porter_dbus::account_path`.
//! `Reauthenticate` and the properties (`Id`, `Provider`, `Label`, `State`, `Capabilities`) are
//! served; every one answers only a caller that holds a grant for the account.
//!
//! An account the caller holds no grant for does not exist for it (`UnknownObject`): there is no
//! bulk enumeration (design/31 §4.4).

use crate::callers::Callers;
use crate::core::{Core, Host, window};
use crate::errors::RefusedError;
use porter_core::consent::Decision;
use porter_core::{Account, AccountId, AccountState, AccountsRequest, AppId, object_segment};
use porter_dbus::{ACCOUNTS_PATH, Details, SheetKind, account_path, to_vardict};
use std::sync::Arc;
use zbus::fdo;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, ObjectServer};

/// One account's object.
#[derive(Debug)]
pub(crate) struct AccountObject<H, C>(Arc<Core<H, C>>);

impl<H, C> AccountObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// The state's slug.
pub(crate) fn state_slug(state: AccountState) -> &'static str {
    match state {
        AccountState::Ok => "ok",
        AccountState::NeedsReauth => "needs_reauth",
        AccountState::Offline => "offline",
        AccountState::Limited => "limited",
    }
}

/// What a property read needs: the account at this path, for a caller that holds a grant on it.
async fn visible<H: Host, C: Callers>(
    core: &Core<H, C>,
    header: Option<&Header<'_>>,
) -> fdo::Result<(Account, AppId)> {
    let header = header.ok_or_else(|| fdo::Error::AccessDenied("no caller".into()))?;
    let app = core
        .caller(header)
        .await
        .map_err(|_| fdo::Error::AccessDenied("accountd does not know this caller".into()))?;
    let registry = core.host.registry();
    let path = header.path().map(|p| p.as_str()).unwrap_or_default();
    account_at(&registry.accounts, path)
        .filter(|account| holds_grant(core.host.as_ref(), &app, &account.id))
        .map(|account| (account.clone(), app))
        .ok_or_else(|| fdo::Error::UnknownObject("no such account for this caller".into()))
}

/// The account an object path stands for, among `accounts`.
fn account_at<'a>(accounts: &'a [Account], path: &str) -> Option<&'a Account> {
    let segment = path.strip_prefix(&format!("{ACCOUNTS_PATH}/account/"))?;
    accounts
        .iter()
        .find(|account| object_segment(&account.id) == segment)
}

/// Whether `app` holds an allowing grant for `account`.
fn holds_grant(host: &impl Host, app: &AppId, account: &AccountId) -> bool {
    host.registry()
        .grants
        .iter()
        .any(|g| g.key.app == *app && g.key.account == *account && g.decision == Decision::Allow)
}

#[zbus::interface(name = "org.quire.Accounts1.Account")]
impl<H: Host, C: Callers> AccountObject<H, C> {
    #[zbus(property)]
    async fn id(&self, #[zbus(header)] header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(visible(&self.0, header.as_ref()).await?.0.id.to_string())
    }

    #[zbus(property)]
    async fn provider(&self, #[zbus(header)] header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(visible(&self.0, header.as_ref())
            .await?
            .0
            .provider
            .to_string())
    }

    #[zbus(property)]
    async fn label(&self, #[zbus(header)] header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(visible(&self.0, header.as_ref()).await?.0.label.0)
    }

    #[zbus(property)]
    async fn state(&self, #[zbus(header)] header: Option<Header<'_>>) -> fdo::Result<String> {
        Ok(state_slug(visible(&self.0, header.as_ref()).await?.0.state).to_owned())
    }

    /// The effective capabilities of the kinds the caller holds an allowing grant for: the kind's
    /// slug and the claim's fields by name.
    #[zbus(property)]
    async fn capabilities(
        &self,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> fdo::Result<Vec<(String, Details)>> {
        let (account, app) = visible(&self.0, header.as_ref()).await?;
        let registry = self.0.host.registry();
        let granted = |kind| {
            registry.grants.iter().any(|g| {
                g.key.app == app
                    && g.key.account == account.id
                    && g.key.kind == kind
                    && g.decision == Decision::Allow
            })
        };
        Ok(account
            .capabilities
            .iter()
            .filter(|claim| granted(claim.offer.kind()))
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
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let app = self.0.acting(&header).await?;
        let registry = self.0.host.registry();
        let path = header.path().map(|p| p.as_str()).unwrap_or_default();
        let account = account_at(&registry.accounts, path)
            .filter(|account| holds_grant(self.0.host.as_ref(), &app, &account.id))
            .map(|account| account.id.clone())
            .ok_or_else(|| RefusedError::unknown_object("no such account for this caller"))?;
        let request = AccountsRequest::Reauthenticate {
            account,
            window: window(&parent_window),
        };
        self.0
            .sheet(
                &header,
                connection,
                SheetKind::Reauthenticate,
                &options,
                request,
            )
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
            .at(account_path(&account.id), AccountObject(Arc::clone(core)))
            .await?;
    }
    Ok(())
}
