//! What every object shares: the service, the caller table, and the checks every method starts
//! with. [`serve`] puts the objects on a connection.

use crate::account::{AccountObject, publish_accounts};
use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::hub::{Event, events, hears, settings_news};
use crate::manager::Manager;
use porter_core::clock::SystemClock;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{AccountId, AccountState, AccountsReply, AccountsRequest, AppId, ProviderId};
use porter_dbus::{ACCOUNTS_BUS, ACCOUNTS_PATH, Caller, CallerRole, account_path};
use porter_families::set::ProviderNames;
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};
use porter_service::{
    AccountService, AuditSink, Clock, Registry, RegistryStore, RevokeReport, Sheets,
};
use porter_sheets::{AppNames, RequestObjects};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use zbus::fdo::Properties;
use zbus::message::Header;
use zbus::names::{BusName, InterfaceName};
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

    /// Signs any account in again for the sheet host or Settings, which need no grant (accountd
    /// checks the role). A host with no sign-in says unavailable.
    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        let _ = (caller, account, window);
        async { AccountsReply::Refused(Refusal::Unavailable) }
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

    /// Records what Settings changed outside the registry (a client id). A host with no audit
    /// records nothing.
    fn audit_settings(&self, event: porter_core::audit::AuditEvent) {
        let _ = event;
    }

    /// Records what `app` did that concerns no account (a desktop-wide Space made, renamed or
    /// removed). A host with no audit records nothing.
    fn audit_app(&self, app: &AppId, event: porter_core::audit::AuditEvent) {
        let _ = (app, event);
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

    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::reauthenticate_any(self, caller, account, window)
    }

    fn remove(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<RevokeReport, SecretsError>> + Send {
        AccountService::remove_with_revoke(self, id)
    }

    fn audit_settings(&self, event: porter_core::audit::AuditEvent) {
        AccountService::audit_settings(self, event);
    }

    fn audit_app(&self, app: &AppId, event: porter_core::audit::AuditEvent) {
        AccountService::audit_app(self, app.clone(), event);
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
    /// A caller reading the person's computers (`Tailnet1`): the only standing a `Terminal` has,
    /// and the method that asks for it checks the role itself.
    Machines,
    /// A caller listing or registering desktop-wide Spaces (`Spaces1.List`, `Register`): the only
    /// standing a `Compositor` has; anyone else is as with `Any`, and `Register` checks the role
    /// itself.
    Spaces,
}

/// The shared state of every object accountd serves.
#[derive(Debug)]
pub(crate) struct Core<H, C> {
    pub(crate) host: Arc<H>,
    pub(crate) callers: C,
    /// The connection the objects are served on, for the signals.
    pub(crate) connection: Connection,
    /// The connections that have called, by unique name: who a unicast signal can reach.
    pub(crate) roster: Mutex<BTreeMap<String, Caller>>,
    /// The registry as clients were last told of it.
    pub(crate) published: Mutex<Registry>,
    /// The user's `clients.toml`, which Settings writes.
    pub(crate) clients: Option<std::path::PathBuf>,
    /// What the settings module calls an app (the "apps using this identity" rows).
    pub(crate) app_names: AppNames,
    /// What the settings module calls a provider and which part of the page it is listed under.
    pub(crate) provider_names: ProviderNames,
    /// The time a property that says when a sign-in ends is read on.
    pub(crate) clock: Arc<dyn Clock>,
    /// The served settings module, which announces an account's new state as `Changed`; set
    /// once the module is served.
    pub(crate) settings: OnceLock<ds_settings::live::Served>,
    /// The sheets open now, one per app and kind (rel-11).
    pub(crate) open_sheets: crate::request::OpenSheets,
    /// The Request objects of the sheet methods (porter-sheets), at `Accounts1`'s root.
    pub(crate) requests: RequestObjects,
    /// The desktop-wide Spaces (`org.quire.Spaces1`).
    pub(crate) spaces: tokio::sync::Mutex<crate::spaces::SpaceBook>,
    /// How Tailscale is followed (`org.quire.Tailnet1`); none lists no computers.
    pub(crate) tailnet: Option<crate::tailnet::TailnetWatch>,
    /// The identity picker and the passwords it may hand over (`org.quire.Identities1`); none
    /// refuses `Offer` and `TakePassword` as unavailable.
    pub(crate) identity_desk: Option<Arc<dyn crate::identity_desk::IdentityDesk>>,
    /// The apps that use each identity (`Identities1.RegisterUse`).
    pub(crate) uses: tokio::sync::Mutex<crate::identities::UseBook>,
    /// What each connection just picked in `Identities1.Offer`.
    pub(crate) picks: crate::identities::Picks,
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
        // The agent launcher is inferd's: accountd holds nothing it asks for.
        if caller.role == CallerRole::AgentLauncher {
            return Err(RefusedError::access_denied(
                "accountd serves no agent launcher; the launcher's interface is inferd's",
            ));
        }
        // The terminal is narrower still: it reads the person's computers and is nothing else.
        if standing != Standing::Machines && caller.role == CallerRole::Terminal {
            return Err(RefusedError::access_denied(
                "the terminal may only read the person's computers",
            ));
        }
        // The compositor registers the Spaces it mints and is nothing else to accountd.
        if standing != Standing::Spaces && caller.role == CallerRole::Compositor {
            return Err(RefusedError::access_denied(
                "the compositor may only list and register Spaces",
            ));
        }
        held(&self.roster).insert(sender.to_string(), caller.clone());
        Ok(caller)
    }

    /// The unique names of the connections that have called and whose caller `wanted` admits:
    /// who a signal meant for some roles reaches.
    pub(crate) fn known_where(&self, wanted: impl Fn(&Caller) -> bool) -> Vec<String> {
        held(&self.roster)
            .iter()
            .filter(|(_, caller)| wanted(caller))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The unique names of the connections that have called, which a broadcast reaches.
    pub(crate) fn known(&self) -> Vec<String> {
        held(&self.roster).keys().cloned().collect()
    }

    /// Tells the clients what changed since they were last told: registers and removes the
    /// `Account` objects, and sends each signal to the connections of the roles it concerns
    /// (the shell and Settings, the only ones that read an account).
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
        self.announce_states(&list, &before, &after).await;
        self.announce_identities_removed(&before, &after).await;
        for event in list {
            let names = self.known_where(|who| hears(who.role, &event));
            for name in names {
                let _ = self.tell(&name, &event).await;
            }
        }
    }

    /// Tells the settings module's listeners (the Settings role is the only one it admits) what
    /// changed in the keys they list, as `Changed(key, value)`:
    ///
    /// - a new state: `accounts.<id>.state` with the state slug, so a sign-in that finishes
    ///   later reaches its pane without a `Set`;
    /// - an account that appeared: its `state` row, and one that went: the same key with
    ///   `removed`, since the module has no signal for "the key set changed" and a pane reads
    ///   the schema again on any `Changed` (detent's `Followed::Changed`);
    /// - a new label: `accounts.<id>.label`.
    async fn announce_states(&self, list: &[Event], before: &Registry, after: &Registry) {
        let Some(module) = self.settings.get() else {
            return;
        };
        for (path, value) in settings_news(list, before, after) {
            let _ = module.changed(&path, &value).await;
        }
    }

    /// Tells the settings module's listeners `Changed(<the key's path>, <word>)`: how an action
    /// row's action went.
    pub(crate) async fn announce_row(&self, key: &crate::settings_keys::Key, word: &str) {
        if let Some(module) = self.settings.get() {
            let path = crate::settings_keys::path(key);
            let _ = module
                .changed(
                    &ds_settings::schema::KeyPath(path),
                    &toml::Value::String(word.to_owned()),
                )
                .await;
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
            Event::NeedsReauth(id) => Manager::<H, C>::needs_reauth(&emitter, path(id)?).await,
            Event::StateChanged(id) => {
                // The new value is read by the receiver, which `State` lets only the shell and
                // Settings do.
                let emitter = SignalEmitter::new(&self.connection, account_path(id))?
                    .set_destination(BusName::try_from(name.to_owned())?);
                Properties::properties_changed(
                    &emitter,
                    InterfaceName::try_from("org.quire.Accounts1.Account")?,
                    HashMap::new(),
                    Cow::Borrowed(&["State"]),
                )
                .await
            }
        }
    }

    /// Forgets a connection that left the bus.
    pub(crate) async fn left(self: &Arc<Self>, name: &str) {
        held(&self.roster).remove(name);
        self.picks.clear(name);
    }
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

/// What `serve_with` adds to the objects every accountd serves.
#[derive(Default)]
pub struct Options {
    /// The user's `clients.toml` (`$XDG_CONFIG_HOME/porter/clients.toml`), which the settings
    /// module writes; none refuses writes.
    pub clients: Option<std::path::PathBuf>,
    /// Where the settings module gets an app's display name; the default names no app, so every
    /// app shows its id.
    pub app_names: AppNames,
    /// Where the settings module gets a provider's label and group; the default knows no
    /// provider, so an account shows its provider's id and is placed by how it signs in.
    pub provider_names: ProviderNames,
    /// The clock the `ReauthReason` property reads; none is the system's.
    pub clock: Option<Arc<dyn Clock>>,
    /// Where the desktop-wide Spaces are kept (`spaces.json` in the registry's directory); the
    /// default keeps them in memory only.
    pub spaces: crate::spaces::SpacesStore,
    /// Tailscale, and how it is followed: the account's state, and `org.quire.Tailnet1`'s
    /// computers and signal. The default follows nothing: no computers are listed.
    pub tailnet: Option<crate::tailnet::TailnetWatch>,
    /// Where the apps that use each identity are kept (`identity-uses.json` in the registry's
    /// directory); the default keeps them in memory only.
    pub uses: crate::identities::UsesStore,
    /// The picker and the passwords behind `org.quire.Identities1`'s `Offer` and `TakePassword`;
    /// none refuses both as unavailable (`List`, `RegisterUse` and `Uses` need no desk).
    pub identities: Option<Arc<dyn crate::identity_desk::IdentityDesk>>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("clients", &self.clients)
            .field("app_names", &self.app_names)
            .field("provider_names", &self.provider_names)
            .field("clock", &self.clock.as_ref().map(|_| "<injected>"))
            .field("spaces", &self.spaces)
            .field("tailnet", &self.tailnet)
            .field("uses", &self.uses)
            .field("identities", &self.identities)
            .finish()
    }
}

/// Serves `org.quire.Accounts1` on `connection` over `host`, answering for the apps `callers`
/// names, and takes the name. One `Account` object is registered per account the registry holds
/// now; the settings module, `Spaces1`, `Identities1` and `Tailnet1` are served.
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
    let (spaces, said) =
        crate::spaces::SpaceBook::open(&options.spaces, crate::spaces::unix_now()).await;
    if let Some(said) = said {
        eprintln!("accountd: {said}");
    }
    let (uses, said) = crate::identities::UseBook::open(&options.uses).await;
    if let Some(said) = said {
        eprintln!("accountd: {said}");
    }
    let clock: Arc<dyn Clock> = options.clock.unwrap_or_else(|| Arc::new(SystemClock));
    let core = Arc::new(Core {
        host,
        callers,
        connection: connection.clone(),
        roster: Mutex::new(BTreeMap::new()),
        published: Mutex::new(published),
        clients: options.clients,
        app_names: options.app_names,
        provider_names: options.provider_names,
        clock,
        settings: OnceLock::new(),
        open_sheets: crate::request::OpenSheets::default(),
        requests: RequestObjects::new(connection.clone(), porter_dbus::ServiceRoot::Accounts),
        spaces: tokio::sync::Mutex::new(spaces),
        tailnet: options.tailnet,
        identity_desk: options.identities,
        uses: tokio::sync::Mutex::new(uses),
        picks: crate::identities::Picks::default(),
    });
    let server: &ObjectServer = connection.object_server();
    server
        .at(ACCOUNTS_PATH, Manager::new(Arc::clone(&core)))
        .await?;
    server
        .at(
            porter_dbus::SPACES_PATH,
            crate::spaces_object::SpacesObject::new(Arc::clone(&core)),
        )
        .await?;
    server
        .at(
            porter_dbus::IDENTITIES_PATH,
            crate::identities_object::IdentitiesObject::new(Arc::clone(&core)),
        )
        .await?;
    server
        .at(
            porter_dbus::TAILNET_PATH,
            crate::tailnet_object::TailnetObject::new(Arc::clone(&core)),
        )
        .await?;
    publish_accounts(server, &core).await?;
    crate::settings::serve_settings(connection, &core).await?;
    crate::roster::watch(connection, Arc::clone(&core)).await?;
    if let Some(watch) = core.tailnet.clone() {
        tokio::spawn(crate::tailnet::follow(Arc::clone(&core), watch));
    }
    // Every object is registered; the name is the promise that calls are taken: claim it only
    // once they are.
    porter_dbus::serve_ready(connection).await?;
    connection.request_name(ACCOUNTS_BUS).await?;
    Ok(())
}
