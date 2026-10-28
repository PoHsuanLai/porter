//! Serving accountd's objects: [`serve`] puts `Manager`, the `Account` objects, `Spaces1`,
//! `Identities1`, `Tailnet1`, the AI keys' `Keys`, `Launcher` and `Broker` and the settings module
//! on a connection over a [`Host`], and takes the name. What the objects share is `state`; this module is the one that names them all.

use crate::account::publish_accounts;
use crate::callers::Callers;
use crate::host::Host;
use crate::manager::Manager;
use crate::state::Core;
use porter_dbus::accounts::{BrokerServed, IdentitiesServed, KeysServed, LauncherServed};
use porter_dbus::accounts::{ManagerServed, SpacesServed, TailnetServed};
use porter_families::set::ProviderNames;
use porter_ids::{clock::Clock, clock::SystemClock};
use porter_sheets::{AppNames, RequestObjects};
use porter_wire::names::{ACCOUNTS_BUS, ACCOUNTS_PATH};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use zbus::{Connection, ObjectServer};

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
    /// The directory the desktop-wide Spaces are kept in (`spaces.json` in the registry's
    /// directory); none, the default, keeps them in memory only.
    pub spaces: Option<std::path::PathBuf>,
    /// Tailscale, and how it is followed: the account's state, and `org.quire.Tailnet1`'s
    /// computers and signal. The default follows nothing: no computers are listed.
    pub tailnet: Option<crate::tailnet::TailnetWatch>,
    /// The directory the apps that use each identity are kept in (`identity-uses.json` in the
    /// registry's directory); none, the default, keeps them in memory only.
    pub uses: Option<std::path::PathBuf>,
    /// The picker and the passwords behind `org.quire.Identities1`'s `Offer` and `TakePassword`;
    /// none refuses both as unavailable (`List`, `RegisterUse` and `Uses` need no desk).
    pub identities: Option<Arc<dyn porter_identities::IdentityDesk>>,
    /// The roster of agent launchers, with the clock, the bound and the audit of a request to
    /// one ([`Launchers::new`](crate::launcher::Launchers::new)); the default is one on the
    /// system clock, ten minutes, one second, and nothing recorded. A host that keeps a handle
    /// on the roster (a clone) can ask a launcher to sign an agent in itself.
    pub launchers: Option<crate::launcher::Launchers>,
    /// Woken after each save of the registry (a [`crate::NewsStore`]); each wake tells the
    /// clients what moved. None leaves the objects to tell them after their own changes.
    pub saved: Option<Arc<tokio::sync::Notify>>,
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
            .field("launchers", &self.launchers)
            .field("saved", &self.saved.as_ref().map(|_| "<set>"))
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
    let (spaces, said) = porter_spaces::SpaceBook::open(options.spaces.as_deref()).await;
    if let Some(said) = said {
        eprintln!("accountd: {said}");
    }
    let (uses, said) = porter_identities::UseBook::open(options.uses.as_deref()).await;
    if let Some(said) = said {
        eprintln!("accountd: {said}");
    }
    let clock: Arc<dyn Clock> = options.clock.unwrap_or_else(|| Arc::new(SystemClock));
    // No launcher session is open in a starting accountd, so a session permit the store holds is
    // one nobody can end.
    host.end_session_grants(None).await;
    let launchers = options.launchers.unwrap_or_else(|| {
        crate::launcher::Launchers::new(connection.clone(), crate::launcher::LoginTiming::new())
    });
    host.use_launcher_roster(launchers.roster());
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
        open_sheets: crate::open_sheets::OpenSheets::default(),
        requests: RequestObjects::new(
            connection.clone(),
            porter_dbus::accounts::ServiceRoot::Accounts,
        ),
        spaces: tokio::sync::Mutex::new(spaces),
        tailnet: options.tailnet,
        identity_desk: options.identities,
        uses: tokio::sync::Mutex::new(uses),
        picks: porter_identities::Picks::default(),
        publisher: OnceLock::new(),
        launchers,
        credentials: crate::launcher::Credentials::default(),
    });
    crate::publish::start(&core);
    if let Some(saved) = options.saved {
        crate::publish::follow_saves(&core, saved);
    }
    let server: &ObjectServer = connection.object_server();
    server
        .at(
            ACCOUNTS_PATH,
            ManagerServed(Manager::new(Arc::clone(&core))),
        )
        .await?;
    server
        .at(
            ACCOUNTS_PATH,
            KeysServed(crate::keys::KeysObject::new(Arc::clone(&core))),
        )
        .await?;
    server
        .at(
            ACCOUNTS_PATH,
            LauncherServed(crate::launcher::LauncherObject::new(Arc::clone(&core))),
        )
        .await?;
    server
        .at(
            ACCOUNTS_PATH,
            BrokerServed(crate::broker::BrokerObject::new(Arc::clone(&core))),
        )
        .await?;
    server
        .at(
            porter_wire::names::SPACES_PATH,
            SpacesServed(crate::spaces_object::SpacesObject::new(Arc::clone(&core))),
        )
        .await?;
    server
        .at(
            porter_wire::names::IDENTITIES_PATH,
            IdentitiesServed(crate::identities_object::IdentitiesObject::new(Arc::clone(
                &core,
            ))),
        )
        .await?;
    server
        .at(
            porter_wire::names::TAILNET_PATH,
            TailnetServed(crate::tailnet_object::TailnetObject::new(Arc::clone(&core))),
        )
        .await?;
    publish_accounts(server, &core).await?;
    crate::settings::serve_settings(connection, &core).await?;
    crate::roster::watch(connection, Arc::clone(&core)).await?;
    if let Some(watch) = core.tailnet.clone() {
        tokio::spawn(crate::tailnet_follow::follow(Arc::clone(&core), watch));
    }
    // Every object is registered; the name is the promise that calls are taken: claim it only
    // once they are.
    porter_dbus::serve_ready(connection).await?;
    connection.request_name(ACCOUNTS_BUS).await?;
    Ok(())
}
