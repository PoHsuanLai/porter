//! `org.quire.Accounts1.Manager`: add an identity through the sheet, and tell the shell which
//! accounts need signing in again. Apps no longer find, choose or are granted accounts here: they
//! ask `Identities1` for "Continue as" and keep their own sign-ins.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing, hint, window};
use crate::errors::RefusedError;
use porter_core::AccountsRequest;
use porter_core::wire::Refusal;
use porter_dbus::Details;
use porter_dbus::callers::CallerRole;
use porter_dbus::names::account_path;
use porter_dbus::sheet::SheetKind;
use std::sync::Arc;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::ObjectPath;
use zbus::zvariant::OwnedObjectPath;

/// The manager object at the accountd path.
#[derive(Debug)]
pub(crate) struct Manager<H, C>(Arc<Core<H, C>>);

impl<H, C> Manager<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

#[zbus::interface(name = "org.quire.Accounts1.Manager")]
impl<H: Host, C: Callers> Manager<H, C> {
    async fn add_account(
        &self,
        #[zbus(header)] header: Header<'_>,
        provider_hint: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let request = AccountsRequest::AddAccount {
            hint: hint(&provider_hint)?,
            window: window(&parent_window),
        };
        self.0
            .sheet(&header, SheetKind::AddAccount, &options, request)
            .await
    }

    /// The accounts that need signing in again now, for the shell alone (`SheetHost`). Asking
    /// joins the caller to the roster, so it is then sent `NeedsReauth` and `State` changes for
    /// every account.
    async fn needing_reauth(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Vec<(OwnedObjectPath, String)>, RefusedError> {
        let caller = self.0.identify(&header, Standing::Any).await?;
        if caller.role != CallerRole::SheetHost {
            return Err(RefusedError::of(Refusal::Denied));
        }
        self.0
            .host
            .registry()
            .accounts
            .iter()
            .filter(|account| account.state.needs_person())
            .map(|account| {
                OwnedObjectPath::try_from(account_path(&account.id))
                    .map(|path| (path, account.label.0.clone()))
                    .map_err(|e| RefusedError::failed(e.to_string()))
            })
            .collect()
    }

    #[zbus(signal)]
    pub(crate) async fn account_added(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn account_removed(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn needs_reauth(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;
}
