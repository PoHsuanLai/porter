//! `org.quire.Accounts1.Account`: one object per account, at `porter_dbus::account_path`.
//! `Reauthenticate` is served; the properties are not (see the crate docs).
//!
//! An account the caller holds no grant for does not exist for it (`UnknownObject`): there is no
//! bulk enumeration (design/31 §4.4).

use crate::callers::Callers;
use crate::core::{Core, Host, window};
use crate::errors::RefusedError;
use porter_core::consent::Decision;
use porter_core::{Account, AccountId, AccountsRequest, AppId, object_segment};
use porter_dbus::{ACCOUNTS_PATH, Details, SheetKind, account_path};
use std::sync::Arc;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, ObjectServer};

/// One account's object.
#[derive(Debug)]
pub(crate) struct AccountObject<H, C>(Arc<Core<H, C>>);

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
    async fn reauthenticate(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let app = self.0.caller(&header).await?;
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
