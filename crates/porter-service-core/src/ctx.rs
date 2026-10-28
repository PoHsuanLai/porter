//! The state every domain shares, and the three things every domain does with it: lock the
//! registry, save it, and record an audited event. The domains (accounts, grants) are traits
//! over a [`Ctx`] in their own crates; `porter_service::AccountService` is the facade a host
//! holds over them.

use crate::audit::{AuditEntry, AuditEvent, AuditSink, NoAudit};
use crate::flight::Flights;
use crate::gate::Gate;
use crate::registry::Registry;
use crate::sheets::Sheets;
use porter_core::store::{NoStore, RegistryStore};
use porter_core::{AccountId, ProviderId};
use porter_core::{AccountState, AppId, AuthKind};
use porter_ids::clock::Clock;
use porter_provider::{Provider, ProviderSet, ProviderSpec};
use porter_secrets::Secrets;
use porter_wire::Refusal;
use std::marker::PhantomData;
use std::sync::{Mutex, MutexGuard};

/// The seams a service is built over, named once: its providers (protocol code), its secrets,
/// the host that draws its sheets, a clock, the store its registry lives in between runs and the
/// sink its audit lines go to. A domain is written for any `E: Ports` instead of repeating the
/// six.
pub trait Ports {
    /// Protocol code: signs an account in and mints its tokens.
    type Provider: Provider;
    /// Where credentials are filed.
    type Secrets: Secrets;
    /// The host that draws consent and sign-in.
    type Sheets: Sheets;
    /// Now.
    type Clock: Clock;
    /// Where the registry lives between runs.
    type Store: RegistryStore;
    /// Where audit lines go.
    type Audit: AuditSink;
}

/// The six seams as type parameters: the one [`Ports`] a host builds. The store and the audit
/// sink default to none: the registry then lives for the process, and nothing is logged.
pub struct Parts<P, S, U, K, R = NoStore, A = NoAudit>(Seams<P, S, U, K, R, A>);

/// The six seams as a marker: `Parts` holds none of them, and is `Send` and `Sync` whatever they are.
type Seams<P, S, U, K, R, A> = PhantomData<fn() -> (P, S, U, K, R, A)>;

impl<P, S, U, K, R, A> std::fmt::Debug for Parts<P, S, U, K, R, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Parts")
    }
}

impl<P, S, U, K, R, A> Ports for Parts<P, S, U, K, R, A>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
{
    type Provider = P;
    type Secrets = S;
    type Sheets = U;
    type Clock = K;
    type Store = R;
    type Audit = A;
}

/// The service's seams and its registry.
pub struct Ctx<E: Ports> {
    providers: Vec<E::Provider>,
    catalog: ProviderSet,
    secrets: E::Secrets,
    sheets: E::Sheets,
    clock: E::Clock,
    store: E::Store,
    audit: E::Audit,
    registry: Mutex<Registry>,
    /// Held across a save's snapshot and write, so saves do not overlap.
    persist_gate: Gate,
    /// Each account's turn at its issuer: one refresh at a time, and the token it issued.
    flights: Flights,
}

impl<E: Ports> std::fmt::Debug for Ctx<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx").finish_non_exhaustive()
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock> Ctx<Parts<P, S, U, K>> {
    /// A context over these providers, starting from `registry` (loaded by the host).
    pub fn new(providers: Vec<P>, registry: Registry, secrets: S, sheets: U, clock: K) -> Self {
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
        }
    }
}

impl<P, S, U, K, R, A> Ctx<Parts<P, S, U, K, R, A>>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
{
    /// The same context saving its registry to `store` after every change.
    pub fn with_store<N: RegistryStore>(self, store: N) -> Ctx<Parts<P, S, U, K, N, A>> {
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
        }
    }

    /// The same context recording its decisions to `audit`.
    pub fn with_audit<N: AuditSink>(self, audit: N) -> Ctx<Parts<P, S, U, K, R, N>> {
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
        }
    }
}

impl<E: Ports> Ctx<E> {
    /// The providers, one per family a spec was given for.
    pub fn providers(&self) -> &[E::Provider] {
        &self.providers
    }

    /// The provider that serves `id`, when the service has one.
    pub fn provider_of(&self, id: &ProviderId) -> Option<&E::Provider> {
        self.providers.iter().find(|p| p.spec().id == *id)
    }

    /// The provider files the service knows.
    pub fn catalog(&self) -> &ProviderSet {
        &self.catalog
    }

    /// Where credentials are filed.
    pub fn secrets(&self) -> &E::Secrets {
        &self.secrets
    }

    /// The host that draws sheets.
    pub fn sheets(&self) -> &E::Sheets {
        &self.sheets
    }

    /// The clock.
    pub fn clock(&self) -> &E::Clock {
        &self.clock
    }

    /// Each account's turn at its issuer.
    pub fn flights(&self) -> &Flights {
        &self.flights
    }

    /// Adds the local runtimes (`AuthKind::LocalRuntime`) among `specs` to the catalogue; any
    /// other spec is ignored (a provider needs its family).
    pub fn add_local_runtimes(&mut self, specs: Vec<ProviderSpec>) {
        let mut all = self.catalog.specs().to_vec();
        let known = |all: &[ProviderSpec], id: &ProviderId| all.iter().any(|s| s.id == *id);
        for spec in specs {
            if spec.auth.kind == AuthKind::LocalRuntime && !known(&all, &spec.id) {
                all.push(spec);
            }
        }
        self.catalog = ProviderSet::layered(all, Vec::new());
    }

    /// The registry, locked for a plain data update.
    pub fn lock(&self) -> MutexGuard<'_, Registry> {
        // Every critical section is a plain data update that cannot panic midway.
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Saves the registry; `Unavailable` when the store cannot be written. The change stays in
    /// memory, and the next save writes it.
    ///
    /// One save runs at a time, and its snapshot is taken only once it holds the gate: a save
    /// that waited writes the registry as it is then, so the last save to finish always carries
    /// the last state (a snapshot taken before the wait could land after a newer one).
    pub async fn persist(&self) -> Result<(), Refusal> {
        let _one_at_a_time = self.persist_gate.lock().await;
        let snapshot = self.lock().persisted();
        self.store
            .save(&snapshot)
            .await
            .map_err(|_| Refusal::Unavailable)
    }

    /// Records one audited event, dated now.
    pub fn note(&self, app: Option<AppId>, account: Option<AccountId>, event: AuditEvent) {
        self.audit
            .record(AuditEntry::new(self.clock.now(), app, account, event));
    }

    /// Sets an account's state (`NeedsReauth` when its credential was refused, `Ok` again once
    /// it is signed in). Whether it changed.
    pub async fn set_state(&self, id: &AccountId, state: AccountState) -> bool {
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
