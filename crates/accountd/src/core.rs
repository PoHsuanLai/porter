//! What every object shares: the service, the caller table, and the checks every method starts
//! with. [`serve`] puts the objects on a connection.

use crate::account::{AccountObject, publish_accounts};
use crate::app_names::AppNames;
use crate::callers::Callers;
use crate::credentials::Credentials;
use crate::errors::RefusedError;
use crate::grants::{Grants, Tokens};
use crate::hub::{Event, audience, events, settings_news, shell_hears};
use crate::keys::KeyDesk;
use crate::launchers::{Launchers, LoginTiming, SignOutNews};
use crate::manager::Manager;
use crate::provider_names::ProviderNames;
use crate::relay::{RelayRoots, Relays};
use porter_core::consent::Usage;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    AccountId, AccountState, AccountsReply, AccountsRequest, AgentState, AppId, AuthKind,
    CapabilityKind, Claim, DataClass, EndpointUrl, GrantId, LauncherSession, Need, ProviderId,
    RelayPlan, Toggle,
};
use porter_dbus::{ACCOUNTS_BUS, ACCOUNTS_PATH, Caller, CallerRole, Details, account_path};
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};
use porter_service::{
    AccountService, AgentFault, AuditSink, Clock, Launchers as LaunchersSeam, LocalFault, Registry,
    RegistryStore, RevokeReport, Roster, Sheets, SyncClass,
};
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::atomic::AtomicU64;
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

    /// Tells the host which agent programs have a launcher, so that adding an agent account
    /// with none ends in `NoLauncher`. A host that adds no agent accounts ignores it.
    fn use_launcher_roster(&self, roster: Roster) {
        let _ = roster;
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

    /// What the relay for `origin` under `caller`'s `grant` is told: no credential, and only an
    /// origin the account's provider file declares as one its links point at. A host with no
    /// relay says unavailable.
    fn open_linked_relay(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send {
        let _ = (caller, grant, origin);
        async { Err(Refusal::Unavailable) }
    }

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

    /// Signs an agent account in through its launcher, with a sheet that shows the wait and the
    /// outcome (`shell` is the sheet host or Settings, which need no grant; an app needs one).
    /// A host with no sign-in says unavailable.
    fn login_agent(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        shell: bool,
        launchers: &impl LaunchersSeam,
    ) -> impl Future<Output = AccountsReply> + Send {
        let _ = (caller, account, window, shell, launchers);
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

    /// Records what Settings changed outside the registry (a client id). A host with no audit
    /// records nothing.
    fn audit_settings(&self, event: porter_core::audit::AuditEvent) {
        let _ = event;
    }

    /// Withdraws any grant, whoever holds it.
    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send {
        let _ = grant;
        async { Err(Refusal::Unavailable) }
    }

    /// The consent sheet for an agent program's key (`Peer.RequestAgentGrant`): `agent` is the
    /// app `org.quire.Agent.<program>` the grant is held under, and `session` an open launcher
    /// session of the caller, which lets the sheet offer "This session only". A host with no
    /// sheets says unavailable.
    fn choose_for_agent(
        &self,
        agent: &AppId,
        need: Need,
        class: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = AccountsReply> + Send {
        let _ = (agent, need, class, window, session);
        async { AccountsReply::Refused(Refusal::Unavailable) }
    }

    /// Removes the grants scoped to `session` (every session grant when none is named) and
    /// returns them. A host that keeps no grants ends none.
    fn end_session_grants(
        &self,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = Vec<GrantId>> + Send {
        let _ = session;
        async { Vec::new() }
    }

    /// Lets syncd keep `class` of an account on this computer, or takes that back (Settings'
    /// sync rows).
    fn set_sync(
        &self,
        id: &AccountId,
        class: SyncClass,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        let _ = (id, class, toggle);
        async { Err(Refusal::Unavailable) }
    }

    /// Sets an account's state; whether it changed.
    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        let _ = (id, state);
        async { false }
    }

    /// Records what an agent program says of its own sign-in (`Peer.SetAgentState`); whether
    /// the account's state changed. A host that keeps no such accounts says unavailable.
    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send {
        let _ = (account, state);
        async { Err(AgentFault::Unavailable) }
    }

    /// Takes a local runtime's report (`Peer.ReportLocal`): the account of `provider`, its
    /// models as claims, its state. A host that keeps no such accounts says unavailable.
    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send {
        let _ = (provider, models, state);
        async { Err(LocalFault::Unavailable) }
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

    fn use_launcher_roster(&self, roster: Roster) {
        AccountService::set_launcher_roster(self, roster);
    }

    fn open_relay(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send {
        AccountService::open_authenticated(self, caller, grant, endpoint)
    }

    fn open_linked_relay(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send {
        AccountService::open_linked(self, caller, grant, origin)
    }

    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::reauthenticate_any(self, caller, account, window)
    }

    async fn login_agent(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        shell: bool,
        launchers: &impl LaunchersSeam,
    ) -> AccountsReply {
        match shell {
            true => AccountService::login_agent_any(self, caller, account, window, launchers).await,
            false => AccountService::login_agent(self, caller, account, window, launchers).await,
        }
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

    fn audit_settings(&self, event: porter_core::audit::AuditEvent) {
        AccountService::audit_settings(self, event);
    }

    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send {
        AccountService::revoke_grant(self, grant)
    }

    fn choose_for_agent(
        &self,
        agent: &AppId,
        need: Need,
        class: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::choose_for_agent(self, agent, need, class, window, session)
    }

    fn end_session_grants(
        &self,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = Vec<GrantId>> + Send {
        AccountService::end_session_grants(self, session)
    }

    fn set_sync(
        &self,
        id: &AccountId,
        class: SyncClass,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        AccountService::set_sync(self, id, class, toggle)
    }

    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        AccountService::set_state(self, id, state)
    }

    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send {
        AccountService::report_local(self, provider, models, state)
    }

    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send {
        AccountService::set_agent_state(self, account, state)
    }
}

/// How much of its role a caller must have for a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Any identified caller.
    Any,
    /// A caller that may act for the person: an `Agent` is refused (`Denied`).
    Acting,
    /// A caller reporting an agent's state: the only standing an `AgentLauncher` has, and the
    /// method that asks for it checks the role itself.
    Launching,
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
    pub(crate) roster: Mutex<BTreeMap<String, Caller>>,
    /// The registry as clients were last told of it.
    pub(crate) published: Mutex<Registry>,
    /// The user's `clients.toml`, which Settings writes.
    pub(crate) clients: Option<std::path::PathBuf>,
    /// The connector the authenticated relays dial with.
    pub(crate) relays: Relays,
    /// Reads the API keys `Peer.ResolveKey` releases; none refuses it `Unavailable`.
    pub(crate) keys: Option<Arc<dyn KeyDesk>>,
    /// What the settings module calls an app.
    pub(crate) app_names: AppNames,
    /// What the settings module calls a provider and which part of the page it is listed under.
    pub(crate) provider_names: ProviderNames,
    /// The agent launchers and the requests they carry.
    pub(crate) launchers: Launchers,
    /// The credentials handed to processes the launcher spawned (`Tokens.IssueProcessCredential`).
    pub(crate) credentials: Credentials,
    /// The served settings module, which announces an account's new state as `Changed`; set
    /// once the module is served.
    pub(crate) settings: OnceLock<ds_settings::live::Served>,
    /// The sheets open now, one per app and kind (rel-11).
    pub(crate) open_sheets: crate::request::OpenSheets,
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
        // The launcher is narrow: it reports an agent's state and is nothing else to accountd.
        if standing != Standing::Launching && caller.role == CallerRole::AgentLauncher {
            return Err(RefusedError::access_denied(
                "the agent launcher may only report an agent's state",
            ));
        }
        held(&self.roster).insert(sender.to_string(), caller.clone());
        Ok(caller)
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

    /// The account a grant is for, as the registry holds it now.
    pub(crate) fn account_of_grant(&self, grant: &GrantId) -> Option<AccountId> {
        self.host
            .registry()
            .grants
            .iter()
            .find(|g| g.id == *grant)
            .map(|g| g.key.account.clone())
    }

    /// Whether `account` is an agent that signs itself in (`AuthKind::AgentLogin`).
    pub(crate) fn is_agent(&self, account: &AccountId) -> bool {
        self.host
            .registry()
            .accounts
            .iter()
            .any(|a| a.id == *account && a.auth == AuthKind::AgentLogin)
    }

    /// Marks `account` as needing reauthentication, and tells the clients.
    pub(crate) async fn needs_reauth(self: &Arc<Self>, account: &AccountId) {
        if self
            .host
            .set_state(account, AccountState::NeedsReauth)
            .await
        {
            self.publish().await;
        }
    }

    /// Tells the clients what changed since they were last told: registers and removes the
    /// `Account` objects, and sends each signal to the connections of the apps it concerns.
    pub(crate) async fn publish(self: &Arc<Self>) {
        let after = self.host.registry();
        // A credential handed to a process does not outlive its grant or its account.
        self.end_credentials(&after).await;
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
        for event in list {
            let apps = audience(&event, &before, &after);
            let names: Vec<String> = held(&self.roster)
                .iter()
                .filter(|(_, who)| {
                    apps.contains(&who.app)
                        || (who.role == CallerRole::SheetHost && shell_hears(&event))
                })
                .map(|(name, _)| name.clone())
                .collect();
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

    /// Tells the settings module's listeners how a sign out went: `Changed("accounts.<id>.sign_out",
    /// <word>)`.
    pub(crate) async fn announce_sign_out(&self, id: &AccountId, news: SignOutNews) {
        self.announce_row(&crate::settings_keys::Key::SignOut(id.clone()), news.slug())
            .await;
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
            Event::CapabilityChanged(id) => {
                Manager::<H, C>::capability_changed(&emitter, path(id)?).await
            }
            Event::NeedsReauth(id) => Manager::<H, C>::needs_reauth(&emitter, path(id)?).await,
            Event::StateChanged(id) => {
                // The new value is read by the receiver, which `State` lets only a holder of a
                // grant, or the shell, do.
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
            Event::GrantChanged(grant) => {
                Manager::<H, C>::grant_changed(&emitter, grant.as_str()).await
            }
        }
    }

    /// Forgets a connection that left the bus. Its launcher sessions close first (their grants
    /// are removed and the credentials under them end `session_closed`, with nobody left to
    /// tell), then what is left of its credentials ends `launcher_gone`.
    pub(crate) async fn left(self: &Arc<Self>, name: &str) {
        held(&self.roster).remove(name);
        for session in self.launchers.left(name) {
            self.close_session(&session, false).await;
        }
        self.credentials_left(name);
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
    /// The user's `clients.toml` (`$XDG_CONFIG_HOME/porter/clients.toml`), which the settings
    /// module writes; none refuses writes.
    pub clients: Option<std::path::PathBuf>,
    /// The certificates an authenticated relay trusts: the platform's, or (a test seam) a
    /// scratch CA alone.
    pub relay_roots: RelayRoots,
    /// Where `Peer.ResolveKey` reads API keys from; none leaves it refusing `Unavailable`.
    pub keys: Option<Arc<dyn KeyDesk>>,
    /// Where the settings module gets an app's display name; the default names no app, so every
    /// app shows its id.
    pub app_names: AppNames,
    /// Where the settings module gets a provider's label and group; the default knows no
    /// provider, so an account shows its provider's id and is placed by how it signs in.
    pub provider_names: ProviderNames,
    /// The clock and bound of a request to an agent launcher; the default is the system clock
    /// and ten minutes.
    pub login: LoginTiming,
    /// `$XDG_RUNTIME_DIR`: where a `tmpfs_file` process credential is written
    /// (`<dir>/porter/agent/<id>/key`, cleared when accountd starts). None refuses that way as
    /// unavailable; the `memfd` way needs no directory.
    pub runtime_dir: Option<std::path::PathBuf>,
}

/// Serves `org.quire.Accounts1` on `connection` over `host`, answering for the apps `callers`
/// names, and takes the name. One `Account` object is registered per account the registry holds
/// now; the settings module and `Peer` are served.
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
    // No launcher session is open in a starting accountd, so a session grant the store holds
    // (accountd died with a session open) is one nobody can end.
    host.end_session_grants(None).await;
    let published = host.registry();
    let core = Arc::new(Core {
        host,
        callers,
        minted: AtomicU64::new(0),
        connection: connection.clone(),
        roster: Mutex::new(BTreeMap::new()),
        published: Mutex::new(published),
        clients: options.clients,
        relays: Relays::new(options.relay_roots),
        keys: options.keys,
        app_names: options.app_names,
        provider_names: options.provider_names,
        launchers: Launchers::new(connection.clone(), options.login),
        credentials: Credentials::new(options.runtime_dir.as_deref()),
        settings: OnceLock::new(),
        open_sheets: crate::request::OpenSheets::default(),
    });
    core.host.use_launcher_roster(core.launchers.roster());
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
    // The name is the promise that calls are taken: claim it only once they are.
    dispatching(connection).await?;
    connection.request_name(ACCOUNTS_BUS).await?;
    Ok(())
}

/// How long one look at whether the connection takes calls may go unanswered before the next.
const DISPATCH_LOOK: std::time::Duration = std::time::Duration::from_millis(50);

/// How long the connection may take to start taking calls.
const DISPATCH_BOUND: std::time::Duration = std::time::Duration::from_secs(30);

/// Waits until `connection` takes method calls. zbus starts a connection's object server on a task
/// of its own the first time it is used, and a call that arrives before that task listens is
/// dropped: no answer, no error, and the caller waits for ever. The look is
/// `org.freedesktop.DBus.Peer.Ping` to the connection itself, through the bus, until it is
/// answered. (inferd's `service::dispatching` is the same.)
async fn dispatching(connection: &Connection) -> zbus::Result<()> {
    let me = connection
        .unique_name()
        .ok_or_else(|| zbus::Error::Failure("the connection has no unique name".into()))?
        .as_str()
        .to_owned();
    let deadline = tokio::time::Instant::now() + DISPATCH_BOUND;
    while tokio::time::Instant::now() < deadline {
        let ping = connection.call_method(
            Some(me.as_str()),
            "/",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        );
        if let Ok(answer) = tokio::time::timeout(DISPATCH_LOOK, ping).await {
            return answer.map(|_| ());
        }
    }
    Err(zbus::Error::InputOutput(Arc::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "accountd's connection took no calls",
    ))))
}
