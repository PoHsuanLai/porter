//! What every object shares: the service, the caller table, and the checks every method starts
//! with. [`serve`] puts the objects on a connection.

use crate::account::publish_accounts;
use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::grants::{Grants, Tokens};
use crate::manager::Manager;
use porter_core::wire::{ParentWindow, ProviderHint};
use porter_core::{AccountsReply, AccountsRequest, AppId, ProviderId};
use porter_dbus::{ACCOUNTS_BUS, ACCOUNTS_PATH, Details};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, AuditSink, Clock, Registry, RegistryStore, Sheets};
use serde::de::DeserializeOwned;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use zbus::message::Header;
use zbus::{Connection, ObjectServer};

/// The service as the bus objects use it: one request in, one reply out, for a caller already
/// identified. [`AccountService`] is the implementation; the trait keeps the zbus objects free
/// of its four seam parameters.
pub trait Host: Send + Sync + 'static {
    /// Answers `request` from `caller`.
    fn handle(
        &self,
        caller: &AppId,
        request: AccountsRequest,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// A copy of the registry, for the objects that mirror accounts.
    fn registry(&self) -> Registry;
}

impl<P, S, U, K, R, A> Host for AccountService<P, S, U, K, R, A>
where
    P: Provider + 'static,
    S: Secrets + 'static,
    U: Sheets + 'static,
    K: Clock + 'static,
    R: RegistryStore + 'static,
    A: AuditSink + 'static,
{
    fn handle(
        &self,
        caller: &AppId,
        request: AccountsRequest,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::handle(self, caller, request)
    }

    fn registry(&self) -> Registry {
        AccountService::registry(self)
    }
}

/// The shared state of every object accountd serves.
#[derive(Debug)]
pub(crate) struct Core<H, C> {
    pub(crate) host: Arc<H>,
    pub(crate) callers: C,
    /// Counts the tokens minted for a call that sent none.
    pub(crate) minted: AtomicU64,
}

impl<H: Host, C: Callers> Core<H, C> {
    /// The app behind the sender of the call, or `AccessDenied`.
    pub(crate) async fn caller(&self, header: &Header<'_>) -> Result<AppId, RefusedError> {
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        self.callers
            .app_of(sender.as_str())
            .await
            .ok_or_else(|| RefusedError::access_denied("accountd does not know this caller"))
    }

    /// A request that answers at once: the reply, or the refusal as its error.
    pub(crate) async fn answer(
        &self,
        header: &Header<'_>,
        request: AccountsRequest,
    ) -> Result<AccountsReply, RefusedError> {
        let app = self.caller(header).await?;
        match self.host.handle(&app, request).await {
            AccountsReply::Refused(refusal) => Err(RefusedError::of(refusal)),
            reply => Ok(reply),
        }
    }
}

/// A closed set's slug argument, parsed at the boundary.
pub(crate) fn slug<T: DeserializeOwned>(text: &str) -> Result<T, RefusedError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| RefusedError::invalid(format!("`{text}`: {e}")))
}

/// The `parent_window` argument: empty is no parent.
pub(crate) fn window(text: &str) -> ParentWindow {
    match text.is_empty() {
        true => ParentWindow::Unparented,
        false => ParentWindow::Handle(text.to_owned()),
    }
}

/// The `provider_hint` argument: empty is the provider list.
pub(crate) fn hint(text: &str) -> Result<ProviderHint, RefusedError> {
    match text.is_empty() {
        true => Ok(ProviderHint::Any),
        false => ProviderId::parse(text)
            .map(ProviderHint::Provider)
            .map_err(RefusedError::invalid),
    }
}

/// The options a sheet method may carry: only `handle_token` is read; an unknown key is ignored.
pub(crate) fn handle_token(options: &Details) -> Option<String> {
    let value = options.get(porter_dbus::OPTION_HANDLE_TOKEN)?;
    String::try_from(value.try_clone().ok()?).ok()
}

/// Serves `org.quire.Accounts1` on `connection` over `host`, answering for the apps `callers`
/// names, and takes the name. One `Account` object is registered per account the registry holds
/// now.
pub async fn serve<H: Host, C: Callers>(
    connection: &Connection,
    host: Arc<H>,
    callers: C,
) -> zbus::Result<()> {
    let core = Arc::new(Core {
        host,
        callers,
        minted: AtomicU64::new(0),
    });
    let server: &ObjectServer = connection.object_server();
    server
        .at(ACCOUNTS_PATH, Manager::new(Arc::clone(&core)))
        .await?;
    server
        .at(ACCOUNTS_PATH, Grants::new(Arc::clone(&core)))
        .await?;
    server
        .at(ACCOUNTS_PATH, Tokens::new(Arc::clone(&core)))
        .await?;
    publish_accounts(server, &core).await?;
    connection.request_name(ACCOUNTS_BUS).await?;
    Ok(())
}
