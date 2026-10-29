//! syncd's own account service, hosted in the test process over fakes: what the daemon builds
//! (`syncd::daemon::OwnService`) with memory for the registry, the secrets and the audit, scripted
//! sheets for the sheet host (wrapped as syncd wraps them, so a sync switch answers its own
//! consent), and the relays the supervisors reach the fake servers through. No accountd, no bus
//! for accounts, no real keyring.
#![allow(dead_code)]

use porter_client::{Grants, IdentitiesError, Porter};
use porter_client_inproc::{Hosted as HostedLink, PorterRelays};
use porter_core::IdentityId;
use porter_fake::{FakeProvider, MemoryStore, RecordingAudit, ScriptedSheets};
use porter_ids::clock::FixedClock;
use porter_secrets::MemorySecrets;
use porter_service::AccountService;
use porter_service_core::registry::Registry;
use porter_wire::ParentWindow;
use std::sync::Arc;
use syncd::keep::{Directory, Intent, Keep, Pick, SyncSheets, sync_app};
use syncd::paths::Paths;
use syncd::service::Hub;

pub type Service = AccountService<
    FakeProvider,
    MemorySecrets,
    SyncSheets<ScriptedSheets>,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

pub type Link = HostedLink<Service, PorterRelays>;

/// The hosted service and the `Accounts` the supervisors use, as syncd (`org.quire.Sync`).
pub struct Hosted {
    pub service: Arc<Service>,
    pub accounts: Arc<Grants<Link>>,
    pub intent: Intent,
    pub store: MemoryStore,
    pub audit: RecordingAudit,
}

/// A service over `provider`, starting from `registry` and `secrets`, and the accounts over it.
pub fn host(provider: FakeProvider, registry: Registry, secrets: MemorySecrets) -> Hosted {
    host_with(provider, registry, secrets, ScriptedSheets::default())
}

/// [`host`], with the sheet host answering as `sheets` says.
pub fn host_with(
    provider: FakeProvider,
    registry: Registry,
    secrets: MemorySecrets,
    sheets: ScriptedSheets,
) -> Hosted {
    let intent = Intent::default();
    let store = MemoryStore::default();
    let audit = RecordingAudit::default();
    let service = Arc::new(
        AccountService::new(
            vec![provider],
            registry,
            secrets,
            SyncSheets::new(sheets, intent.clone()),
            FixedClock(porter_fake::NOW),
        )
        .with_store(store.clone())
        .with_audit(audit.clone()),
    );
    let link = HostedLink::new(Arc::clone(&service), sync_app()).with_relays(PorterRelays::new());
    Hosted {
        accounts: Arc::new(Porter::over(link).grants()),
        service,
        intent,
        store,
        audit,
    }
}

/// The identities of a computer where nobody is signed in: the picker finds none.
pub struct NoDirectory;

impl Directory for NoDirectory {
    async fn offer(&self, _window: &ParentWindow) -> Result<Pick, IdentitiesError> {
        Err(IdentitiesError::NoIdentity)
    }

    async fn register_use(&self, _id: &IdentityId) -> Result<(), IdentitiesError> {
        Ok(())
    }
}

/// syncd's accounts over `hosted`, continuing as what `directory` offers; a removal wipes what
/// `paths` and `hub` name.
pub fn keep<D: Directory>(
    hosted: &Hosted,
    directory: D,
    paths: Paths,
    hub: Hub,
) -> Keep<Service, D> {
    Keep::new(
        Arc::clone(&hosted.service),
        directory,
        hosted.intent.clone(),
        paths,
        hub,
    )
}
