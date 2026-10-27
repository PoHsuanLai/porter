//! The state every part of the service shares, and the three things every part does with it:
//! lock the registry, save it, and record an audited event. The domain modules (`add_flow`,
//! `issue`, `grants`, `manage`, `relay`, ...) are methods of [`Ctx`] and call each other down
//! the module order, never back up; [`AccountService`](crate::AccountService) is the facade a
//! host holds over them.

use crate::audit::{AuditEntry, AuditEvent, AuditSink, NoAudit};
use crate::flight::Flights;
use crate::gate::Gate;
use crate::launcher::Roster;
use crate::own_machines::OwnMachines;
use crate::registry::Registry;
use crate::sheets::Sheets;
use porter_core::clock::Clock;
use porter_core::store::{NoStore, RegistryStore};
use porter_core::wire::Refusal;
use porter_core::{AccountId, AccountState, AppId, AuthKind, ProviderId};
use porter_provider::{Provider, ProviderSet, ProviderSpec};
use porter_secrets::Secrets;
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// The service's seams and its registry. The store and the audit sink default to none: the
/// registry then lives for the process, and nothing is logged.
#[derive(Debug)]
pub(crate) struct Ctx<P, S, U, K, R = NoStore, A = NoAudit> {
    pub(crate) providers: Vec<P>,
    pub(crate) catalog: ProviderSet,
    pub(crate) secrets: S,
    pub(crate) sheets: U,
    pub(crate) clock: K,
    pub(crate) store: R,
    pub(crate) audit: A,
    pub(crate) registry: Mutex<Registry>,
    /// Held across a save's snapshot and write, so saves do not overlap.
    pub(crate) persist_gate: Gate,
    /// Each account's turn at its issuer: one refresh at a time, and the token it issued.
    pub(crate) flights: Flights,
    /// Account ids an add has chosen and is still filing secrets for: not in the registry yet
    /// (the row is added only once its secrets are stored), and taken all the same.
    pub(crate) reserved: Mutex<HashSet<AccountId>>,
    /// Which agent programs have a launcher, once the host says (accountd, when it serves).
    pub(crate) roster: OnceLock<Roster>,
    /// The person's computers on their tailnet, once the host says (inferd, which has a view of
    /// the tailnet): an AI server's address may be plain `http` to one of them.
    pub(crate) own_machines: OnceLock<OwnMachines>,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock> Ctx<P, S, U, K> {
    /// A context over these providers, starting from `registry` (loaded by the host).
    pub(crate) fn new(
        providers: Vec<P>,
        registry: Registry,
        secrets: S,
        sheets: U,
        clock: K,
    ) -> Self {
        let specs = providers.iter().map(|p| p.spec().clone()).collect();
        let catalog = ProviderSet::layered(specs, Vec::new());
        Self {
            providers,
            catalog,
            secrets,
            sheets,
            clock,
            store: NoStore,
            audit: NoAudit,
            registry: Mutex::new(registry),
            persist_gate: Gate::default(),
            flights: Flights::default(),
            reserved: Mutex::new(HashSet::new()),
            roster: OnceLock::new(),
            own_machines: OnceLock::new(),
        }
    }
}

impl<P, S, U, K, R, A> Ctx<P, S, U, K, R, A> {
    /// The same context saving its registry to `store` after every change.
    pub(crate) fn with_store<N: RegistryStore>(self, store: N) -> Ctx<P, S, U, K, N, A> {
        Ctx {
            providers: self.providers,
            catalog: self.catalog,
            secrets: self.secrets,
            sheets: self.sheets,
            clock: self.clock,
            store,
            audit: self.audit,
            registry: self.registry,
            persist_gate: self.persist_gate,
            flights: self.flights,
            reserved: self.reserved,
            roster: self.roster,
            own_machines: self.own_machines,
        }
    }

    /// The same context recording its decisions to `audit`.
    pub(crate) fn with_audit<N: AuditSink>(self, audit: N) -> Ctx<P, S, U, K, R, N> {
        Ctx {
            providers: self.providers,
            catalog: self.catalog,
            secrets: self.secrets,
            sheets: self.sheets,
            clock: self.clock,
            store: self.store,
            audit,
            registry: self.registry,
            persist_gate: self.persist_gate,
            flights: self.flights,
            reserved: self.reserved,
            roster: self.roster,
            own_machines: self.own_machines,
        }
    }

    /// Adds the local runtimes (`AuthKind::LocalRuntime`) among `specs` to the catalogue; any
    /// other spec is ignored (a provider needs its family).
    pub(crate) fn add_local_runtimes(&mut self, specs: Vec<ProviderSpec>) {
        let mut all = self.catalog.specs().to_vec();
        let known = |all: &[ProviderSpec], id: &ProviderId| all.iter().any(|s| s.id == *id);
        for spec in specs {
            if spec.auth.kind == AuthKind::LocalRuntime && !known(&all, &spec.id) {
                all.push(spec);
            }
        }
        self.catalog = ProviderSet::layered(all, Vec::new());
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Registry> {
        // Every critical section is a plain data update that cannot panic midway.
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    Ctx<P, S, U, K, R, A>
{
    /// Saves the registry; `Unavailable` when the store cannot be written. The change stays in
    /// memory, and the next save writes it.
    ///
    /// One save runs at a time, and its snapshot is taken only once it holds the gate: a save
    /// that waited writes the registry as it is then, so the last save to finish always carries
    /// the last state (a snapshot taken before the wait could land after a newer one).
    pub(crate) async fn persist(&self) -> Result<(), Refusal> {
        let _one_at_a_time = self.persist_gate.lock().await;
        let snapshot = self.lock().persisted();
        self.store
            .save(&snapshot)
            .await
            .map_err(|_| Refusal::Unavailable)
    }

    /// Records one audited event, dated now.
    pub(crate) fn note(&self, app: Option<AppId>, account: Option<AccountId>, event: AuditEvent) {
        self.audit
            .record(AuditEntry::new(self.clock.now(), app, account, event));
    }

    /// The body of [`AccountService::set_state`](crate::AccountService::set_state).
    pub(crate) async fn set_state(&self, id: &AccountId, state: AccountState) -> bool {
        let changed = {
            let mut registry = self.lock();
            registry
                .accounts
                .iter_mut()
                .find(|a| a.id == *id)
                .is_some_and(|a| std::mem::replace(&mut a.state, state) != state)
        };
        if changed {
            let _ = self.persist().await;
        }
        changed
    }
}
