//! syncd's own account service, hosted in the test process over fakes: what the daemon builds
//! (`syncd::daemon::OwnService`) with memory for the registry, the secrets and the audit, scripted
//! sheets for the sheet host, and the relays the supervisors reach the fake servers through. No
//! accountd, no bus for accounts, no real keyring.
#![allow(dead_code)]

use porter_client::{Accounts, InProcess, NoBroker, PorterRelays};
use porter_fake::{FakeProvider, FixedClock, MemoryStore, RecordingAudit, ScriptedSheets};
use porter_secrets::MemorySecrets;
use porter_service::{AccountService, Registry};
use std::sync::Arc;
use syncd::keep::sync_app;

pub type Service = AccountService<
    FakeProvider,
    MemorySecrets,
    ScriptedSheets,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

pub type Link = InProcess<Service, NoBroker, PorterRelays>;

/// The hosted service and the `Accounts` the supervisors use, as syncd (`org.quire.Sync`).
pub struct Hosted {
    pub service: Arc<Service>,
    pub accounts: Arc<Accounts<Link>>,
}

/// A service over `provider`, starting from `registry` and `secrets`, and the accounts over it.
pub fn host(provider: FakeProvider, registry: Registry, secrets: MemorySecrets) -> Hosted {
    let service = Arc::new(
        AccountService::new(
            vec![provider],
            registry,
            secrets,
            ScriptedSheets::default(),
            FixedClock(porter_fake::NOW),
        )
        .with_store(MemoryStore::default())
        .with_audit(RecordingAudit::default()),
    );
    let link = InProcess::new(Arc::clone(&service), sync_app()).with_relays(PorterRelays::new());
    Hosted {
        accounts: Arc::new(Accounts::over(link)),
        service,
    }
}
