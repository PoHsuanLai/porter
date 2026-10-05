//! What every object shares: the service, the caller table, and the checks every method starts
//! with. [`serve`] puts the objects on a connection.

use crate::account::{AccountObject, publish_accounts};
use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::grants::{Grants, Tokens};
use crate::hub::{Event, audience, events};
use crate::legacy::AdoptConfig;
use crate::manager::Manager;
use crate::relay::{RelayRoots, Relays};
use porter_core::wire::{LegacyRef, ParentWindow, ProviderHint, Refusal};
use porter_core::{
    AccountId, AccountState, AccountsReply, AccountsRequest, AppId, CapabilityKind, EndpointUrl,
    GrantId, ProviderId, RelayPlan, Toggle,
};
use porter_dbus::{ACCOUNTS_BUS, ACCOUNTS_PATH, Caller, CallerRole, Details, account_path};
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};
use porter_service::{
    AccountService, AuditSink, Clock, LegacyStore, Registry, RegistryStore, RevokeReport, Sheets,
};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, PoisonError};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::ObjectPath;
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

    /// Adopts a legacy account from `store` under the legacy `service`. A host with no legacy
    /// store refuses.
    fn adopt_from(
        &self,
        store: &dyn LegacyStore,
        service: &str,
        caller: &AppId,
        legacy: LegacyRef,
    ) -> impl Future<Output = AccountsReply> + Send {
        let _ = (store, service, caller, legacy);
        async { AccountsReply::Refused(Refusal::Unavailable) }
    }

    /// What the relay for `endpoint` under `caller`'s `grant` presents, once the grant and the
    /// endpoint are checked. A host with no relay says unavailable.
    fn open_relay(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send {
        let _ = (caller, grant, endpoint);
        async { Err(Refusal::Unavailable) }
    }

    /// Removes an account: revoke at the provider (best effort), then every wipe. A host that
    /// cannot manage accounts says its secret store is unavailable.
    fn remove(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<RevokeReport, SecretsError>> + Send {
        let _ = id;
        async { Err(SecretsError::Unavailable) }
    }

    /// Switches one kind of one account on or off.
    fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        let _ = (id, kind, toggle);
        async { Err(Refusal::Unavailable) }
    }

    /// Withdraws any grant, whoever holds it.
    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send {
        let _ = grant;
        async { Err(Refusal::Unavailable) }
    }

    /// Sets an account's state; whether it changed.
    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        let _ = (id, state);
        async { false }
    }
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

    fn adopt_from(
        &self,
        store: &dyn LegacyStore,
        service: &str,
        caller: &AppId,
        legacy: LegacyRef,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::adopt_from(self, store, service, caller, legacy)
    }

    fn open_relay(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send {
        AccountService::open_authenticated(self, caller, grant, endpoint)
    }

    fn remove(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<RevokeReport, SecretsError>> + Send {
        AccountService::remove_with_revoke(self, id)
    }

    fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        AccountService::set_toggle(self, id, kind, toggle)
    }

    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send {
        AccountService::revoke_grant(self, grant)
    }

    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        AccountService::set_state(self, id, state)
    }
}

/// How much of its role a caller must have for a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Any identified caller.
    Any,
    /// A caller that may act for the person: an `Agent` is refused (`Denied`).
    Acting,
}

/// The shared state of every object accountd serves.
#[derive(Debug)]
pub(crate) struct Core<H, C> {
    pub(crate) host: Arc<H>,
    pub(crate) callers: C,
    /// Counts the tokens minted for a call that sent none.
    pub(crate) minted: AtomicU64,
    /// The connection the objects are served on, for the signals.
    pub(crate) connection: Connection,
    /// The connections that have called, by unique name: who a unicast signal can reach.
    pub(crate) roster: Mutex<BTreeMap<String, AppId>>,
    /// The registry as clients were last told of it.
    pub(crate) published: Mutex<Registry>,
    /// What `Adopt` reads and who may ask.
    pub(crate) adopt: AdoptConfig,
    /// The user's `clients.toml`, which Settings writes.
    pub(crate) clients: Option<std::path::PathBuf>,
    /// The connector the authenticated relays dial with.
    pub(crate) relays: Relays,
}

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Every critical section is a plain data update.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<H: Host, C: Callers> Core<H, C> {
    /// Who the sender of the call is, with its role; a caller that must `Act` is refused as an
    /// `Agent`. An unknown sender is `AccessDenied`. The caller joins the roster.
    pub(crate) async fn identify(
        &self,
        header: &Header<'_>,
        standing: Standing,
    ) -> Result<Caller, RefusedError> {
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        let caller = self
            .callers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(|| RefusedError::access_denied("accountd does not know this caller"))?;
        if standing == Standing::Acting && caller.role == CallerRole::Agent {
            return Err(RefusedError::of(Refusal::Denied));
        }
        held(&self.roster).insert(sender.to_string(), caller.app.clone());
        Ok(caller)
    }

    /// The app behind the sender of the call, or `AccessDenied`.
    pub(crate) async fn caller(&self, header: &Header<'_>) -> Result<AppId, RefusedError> {
        self.identify(header, Standing::Any).await.map(|c| c.app)
    }

    /// The app behind the sender of a call an `Agent` may not make.
    pub(crate) async fn acting(&self, header: &Header<'_>) -> Result<AppId, RefusedError> {
        self.identify(header, Standing::Acting).await.map(|c| c.app)
    }

    /// A request that answers at once: the reply, or the refusal as its error.
    pub(crate) async fn answer(
        self: &Arc<Self>,
        header: &Header<'_>,
        standing: Standing,
        request: AccountsRequest,
    ) -> Result<AccountsReply, RefusedError> {
        let app = self.identify(header, standing).await?.app;
        let reply = self.host.handle(&app, request).await;
        self.publish().await;
        match reply {
            AccountsReply::Refused(refusal) => Err(RefusedError::of(refusal)),
            reply => Ok(reply),
        }
    }

    /// Marks the account `grant` is for as needing reauthentication, and tells the clients.
    pub(crate) async fn needs_reauth(self: &Arc<Self>, grant: &GrantId) {
        let account = self
            .host
            .registry()
            .grants
            .iter()
            .find(|g| g.id == *grant)
            .map(|g| g.key.account.clone());
        if let Some(account) = account
            && self
                .host
                .set_state(&account, AccountState::NeedsReauth)
                .await
        {
            self.publish().await;
        }
    }

    /// Tells the clients what changed since they were last told: registers and removes the
    /// `Account` objects, and sends each signal to the connections of the apps it concerns.
    pub(crate) async fn publish(self: &Arc<Self>) {
        let after = self.host.registry();
        let before = std::mem::replace(&mut *held(&self.published), after.clone());
        let list = events(&before, &after);
        let server = self.connection.object_server();
        for event in &list {
            match event {
                Event::Added(id) => {
                    let _ = server
                        .at(account_path(id), AccountObject::new(Arc::clone(self)))
                        .await;
                }
                Event::Removed(id) => {
                    let _ = server
                        .remove::<AccountObject<H, C>, _>(account_path(id))
                        .await;
                }
                _ => {}
            }
        }
        for event in list {
            let apps = audience(&event, &before, &after);
            let names: Vec<String> = held(&self.roster)
                .iter()
                .filter(|(_, app)| apps.contains(app))
                .map(|(name, _)| name.clone())
                .collect();
            for name in names {
                let _ = self.tell(&name, &event).await;
            }
        }
    }

    /// Sends `event` to the connection `name` alone.
    async fn tell(&self, name: &str, event: &Event) -> zbus::Result<()> {
        let emitter = SignalEmitter::new(&self.connection, ACCOUNTS_PATH)?
            .set_destination(BusName::try_from(name.to_owned())?);
        let path = |id: &AccountId| ObjectPath::try_from(account_path(id));
        match event {
            Event::Added(id) => Manager::<H, C>::account_added(&emitter, path(id)?).await,
            Event::Removed(id) => Manager::<H, C>::account_removed(&emitter, path(id)?).await,
            Event::CapabilityChanged(id) => {
                Manager::<H, C>::capability_changed(&emitter, path(id)?).await
            }
            Event::NeedsReauth(id) => Manager::<H, C>::needs_reauth(&emitter, path(id)?).await,
            Event::GrantChanged(grant) => {
                Manager::<H, C>::grant_changed(&emitter, grant.as_str()).await
            }
        }
    }

    /// Forgets a connection that left the bus.
    pub(crate) fn left(&self, name: &str) {
        held(&self.roster).remove(name);
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

/// What `serve_with` adds to the objects every accountd serves.
#[derive(Debug, Default)]
pub struct Options {
    /// The legacy store `Manager.Adopt` reads and the apps allowed to use it.
    pub adopt: AdoptConfig,
    /// The user's `clients.toml` (`$XDG_CONFIG_HOME/porter/clients.toml`), which the settings
    /// module writes; none refuses writes.
    pub clients: Option<std::path::PathBuf>,
    /// The certificates an authenticated relay trusts: the platform's, or (a test seam) a
    /// scratch CA alone.
    pub relay_roots: RelayRoots,
}

/// Serves `org.quire.Accounts1` on `connection` over `host`, answering for the apps `callers`
/// names, and takes the name. One `Account` object is registered per account the registry holds
/// now; `Adopt` is refused (no legacy store) and the settings module and `Peer` are served.
pub async fn serve<H: Host, C: Callers>(
    connection: &Connection,
    host: Arc<H>,
    callers: C,
) -> zbus::Result<()> {
    serve_with(connection, host, callers, Options::default()).await
}

/// [`serve`] with `options`.
pub async fn serve_with<H: Host, C: Callers>(
    connection: &Connection,
    host: Arc<H>,
    callers: C,
    options: Options,
) -> zbus::Result<()> {
    let published = host.registry();
    let core = Arc::new(Core {
        host,
        callers,
        minted: AtomicU64::new(0),
        connection: connection.clone(),
        roster: Mutex::new(BTreeMap::new()),
        published: Mutex::new(published),
        adopt: options.adopt,
        clients: options.clients,
        relays: Relays::new(options.relay_roots),
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
    server
        .at(ACCOUNTS_PATH, crate::peer::Peer::new(Arc::clone(&core)))
        .await?;
    publish_accounts(server, &core).await?;
    crate::settings::serve_settings(connection, &core).await?;
    crate::roster::watch(connection, Arc::clone(&core)).await?;
    connection.request_name(ACCOUNTS_BUS).await?;
    Ok(())
}
