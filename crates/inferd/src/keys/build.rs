//! Building the hosted service: the provider files inferd serves (the AI and agent ones), the
//! families of each, its registry, its secret store and its sheets. Used by the daemon
//! ([`open`]) and by `inferd add` ([`load_families`]).

use super::audit::KeyAudit;
use super::desk::SecretsDesk;
use super::local::LocalKeys;
use super::news::NewsStore;
use crate::config::Dirs;
use crate::tailnet::{BusMachines, Machines};
use porter_core::clock::SystemClock;
use porter_core::sheet::ProviderGroup;
use porter_core::store::RegistryStore;
use porter_dbus::callers::{CallerTable, Callers};
use porter_families::set::{FamilyIo, Layer, ProviderNames, family_of, load_specs, local_runtimes};
use porter_families::{ClientFiles, FamilyProvider};
use porter_provider::{Provider, ProviderSet, ProviderSpec};
use porter_secrets::Namespace;
use porter_secrets::keysel::{AnyKeys, Chosen, KeysRefusal, Selection};
use porter_service::{AccountService, OwnMachines, Registry};
use porter_sheets::{AppNames, BusSheets};
use porter_store::FileStore;
use std::sync::Arc;
use tokio::sync::Notify;

/// The keyring namespace inferd files its keys under.
pub const NAMESPACE: &str = "porter.inferd";

/// Why the key store did not open.
#[derive(Debug, thiserror::Error)]
pub enum KeysStartError {
    /// The secret store asked for is refused.
    #[error("{0}")]
    Keys(#[from] KeysRefusal),
    /// The registry file was refused. The text says so in plain words: both files, nothing
    /// changed.
    #[error("{0}")]
    Registry(String),
}

/// The service the daemon hosts: AI and agent families, the shell's sheets, the file registry
/// that says when it was saved.
pub type KeyService<C> = AccountService<
    FamilyProvider,
    AnyKeys,
    BusSheets<C>,
    SystemClock,
    NewsStore<FileStore>,
    KeyAudit,
>;

/// The provider files inferd serves, and the families of each.
#[derive(Debug)]
pub struct Families {
    /// The family of each provider served.
    pub families: Vec<FamilyProvider>,
    /// The local runtimes (Ollama, llama.cpp, LM Studio): no family, reported by the probe.
    pub local: Vec<ProviderSpec>,
    /// What a person reads for each provider, and which group of the page it is under.
    pub names: ProviderNames,
    /// The provider files, for naming agents on a sheet.
    pub specs: Vec<ProviderSpec>,
}

/// Loads the provider files of `dirs` and keeps the AI and agent ones: an identity provider is
/// accountd's, not inferd's.
pub fn load_families(dirs: &Dirs) -> Families {
    let loaded = load_specs(&[
        (Layer::Shipped, dirs.providers_system.clone()),
        (Layer::Person, dirs.providers_own.clone()),
    ]);
    for (file, why) in &loaded.skipped {
        eprintln!("inferd: skipped provider file {}: {why}", file.display());
    }
    let specs: Vec<ProviderSpec> = loaded
        .specs
        .into_iter()
        .filter(|spec| {
            matches!(
                spec.group(),
                ProviderGroup::Intelligence | ProviderGroup::Agent
            )
        })
        .collect();
    let io = FamilyIo::system(
        ProviderSet::layered(specs.clone(), Vec::new()),
        ClientFiles {
            shipped: dirs.clients_shipped.clone(),
            own: dirs.clients_own.clone(),
        },
        Arc::new(SystemClock),
    );
    let (mut families, mut unserved) = (Vec::new(), Vec::new());
    for spec in specs.clone() {
        match family_of(spec, &io) {
            Ok(family) => families.push(family),
            Err(spec) => unserved.push(*spec),
        }
    }
    let local = local_runtimes(&unserved);
    for spec in unserved.iter().filter(|s| !local.contains(s)) {
        eprintln!("inferd: no family serves provider `{}` yet", spec.id);
    }
    Families {
        families,
        local,
        names: ProviderNames::from_specs(&specs),
        specs,
    }
}

/// Reads the registry in `store`: a file it cannot read stops the start, and nothing is changed.
pub async fn load_registry(store: &FileStore) -> Result<Registry, KeysStartError> {
    match store.load().await {
        Ok(stored) => Ok(Registry::from_persisted(stored)),
        Err(why) => Err(KeysStartError::Registry(store.refusal(&why))),
    }
}

/// The key store, opened.
pub struct Opened<C> {
    /// The hosted service.
    pub service: Arc<KeyService<C>>,
    /// The accounts as the cloud turns ask for them.
    pub local: LocalKeys<KeyService<C>>,
    /// What a person reads for each app.
    pub apps: AppNames,
    /// What a person reads for each provider.
    pub providers: ProviderNames,
    /// Woken after each save of the registry.
    pub saved: Arc<Notify>,
}

impl<C> std::fmt::Debug for Opened<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened").finish_non_exhaustive()
    }
}

/// Opens the key store: the secret store `selection` names (refused here, not later), the
/// provider files, the registry, the sheets drawn through `connection` for the shell the table
/// names.
pub async fn open<C: Callers>(
    dirs: &Dirs,
    selection: Selection,
    table: CallerTable,
    connection: zbus::Connection,
    callers: Arc<C>,
) -> Result<Opened<C>, KeysStartError> {
    let namespace = Namespace::parse(NAMESPACE).expect("porter.inferd is a well-formed namespace");
    let chosen = Chosen::open(selection, namespace, "inferd")?;
    eprintln!("{}", chosen.said);
    let secrets = chosen.keys;
    let Families {
        families,
        local,
        names,
        specs,
    } = load_families(dirs);
    let apps = AppNames::new(table, dirs.applications.clone()).with_agents(&specs);
    let file = FileStore::new(dirs.keys.clone());
    let registry = load_registry(&file).await?;
    let saved = Arc::new(Notify::new());
    // The person's computers on their tailnet, as accountd lists them: an add sheet for a server
    // of their own accepts plain http to one of them.
    let tailnet = Arc::new(BusMachines::new(connection.clone()));
    let sheets = BusSheets::new(connection, callers).with_names(apps.clone());
    let service = Arc::new(
        AccountService::new(families, registry, secrets.clone(), sheets, SystemClock)
            .with_local_runtimes(local)
            .with_store(NewsStore::new(file, Arc::clone(&saved)))
            .with_audit(KeyAudit::new(dirs.keys_audit.clone())),
    );
    service.set_own_machines(OwnMachines::new(move || {
        let tailnet = Arc::clone(&tailnet);
        async move { tailnet.machines().await }
    }));
    let desk = SecretsDesk::new(secrets, KeyAudit::new(dirs.keys_audit.clone()), SystemClock);
    let local = LocalKeys::new(Arc::clone(&service), Arc::new(desk), names.clone());
    Ok(Opened {
        service,
        local,
        apps,
        providers: names,
        saved,
    })
}

/// Whether `provider` of `families` is served (what `inferd add` checks before it asks).
pub fn served_ids(families: &[FamilyProvider]) -> Vec<porter_core::ProviderId> {
    families
        .iter()
        .map(|family| Provider::spec(family).id.clone())
        .collect()
}
