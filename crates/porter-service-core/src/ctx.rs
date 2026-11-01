//! The state every domain shares, and the three things every domain does with it: read the
//! registry, change it through [`Ctx::transact`] (the one place it is saved and the one place it
//! is written), and record an audited event. The domains (accounts, grants) are traits
//! over a [`Ctx`] in their own crates; `porter_service::AccountService` is the facade a host
//! holds over them.

use crate::audit::{AuditEntry, AuditEvent, AuditSink, NoAudit};
use crate::flight::Flights;
use crate::gate::Gate;
use crate::registry::Registry;
use porter_core::store::{NoStore, RegistryStore};
use porter_core::{AccountId, ProviderId};
use porter_core::{AccountState, AppId, AuthKind};
use porter_ids::clock::Clock;
use porter_provider::{Provider, ProviderSet, ProviderSpec};
use porter_secrets::Secrets;
use porter_sheet::host::Sheets;
use porter_wire::Refusal;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

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

/// The registry, read: what [`Ctx::lock`] hands out. There is no way to change the registry
/// through it; a change is a [`Ctx::transact`].
#[derive(Debug)]
pub struct Read<'a>(MutexGuard<'a, Registry>);

impl Deref for Read<'_> {
    type Target = Registry;

    fn deref(&self) -> &Registry {
        &self.0
    }
}

/// One audit line a decision asks [`Ctx::transact`] to write once its change is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audited {
    /// Who caused it (none for what accountd does by itself).
    pub app: Option<AppId>,
    /// The account it concerns, if any.
    pub account: Option<AccountId>,
    /// What happened.
    pub event: AuditEvent,
}

impl Audited {
    /// A line for `event`, caused by `app` and concerning `account` where there are any.
    pub fn new(app: Option<AppId>, account: Option<AccountId>, event: AuditEvent) -> Self {
        Self {
            app,
            account,
            event,
        }
    }
}

/// What a transaction that ended well hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written<T> {
    /// What the decision returned.
    pub value: T,
    /// Whether the registry changed. When it did not, nothing was saved or announced.
    pub changed: bool,
}

/// One saved change: the registry before it and after it. Sent to whoever the host named with
/// [`Ctx::on_committed`], once the change is saved, swapped in and audited, in the order the
/// transactions ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    /// The registry the decision started from.
    pub before: Registry,
    /// The registry it left, which is the one now held and saved.
    pub after: Registry,
}

/// Who is told of each [`Committed`]. It runs inside the transaction (the gate is held), so it
/// must not wait.
type Told = Arc<dyn Fn(Committed) + Send + Sync>;

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
    /// Held for the whole of a transaction, so writers go one at a time.
    persist_gate: Gate,
    /// Who is told of each change: the ones the builder named and the ones that asked later.
    committed: Mutex<Vec<Told>>,
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
            committed: Mutex::new(Vec::new()),
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
            committed: self.committed,
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
            committed: self.committed,
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

    /// Tells `told` of every change once it is saved (see [`Committed`]).
    #[must_use]
    pub fn on_committed(self, told: impl Fn(Committed) + Send + Sync + 'static) -> Self {
        self.follow(told);
        self
    }

    /// Tells `told` of every change from now on, as [`Ctx::on_committed`] does, for a host that
    /// serves the service it was handed and so cannot name its listener when it builds it.
    pub fn follow(&self, told: impl Fn(Committed) + Send + Sync + 'static) {
        self.committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Arc::new(told));
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        // Every critical section is a plain data update that cannot panic midway.
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The registry, to read. Hold it only while reading, never across an `.await`.
    pub fn lock(&self) -> Read<'_> {
        Read(self.registry())
    }

    /// Changes the registry: the one way it is changed.
    ///
    /// 1. Takes the gate, so one transaction runs at a time and each starts from the registry the
    ///    last one left.
    /// 2. Runs `decide` on a copy of the registry (`decide` is sync, so no await is in it; an
    ///    effect it needs ran before this call, and `decide` checks again against the registry
    ///    as it is now). Its `Err` ends the transaction with nothing changed.
    /// 3. A copy equal to the registry ends it too: nothing is saved or announced, and the lines
    ///    `decide` returned are written (a decision that changed nothing and wants no line returns
    ///    none).
    /// 4. Saves the copy. A store that cannot be written is `Unavailable`, and the registry, the
    ///    audit log and the listeners are as they were.
    /// 5. Swaps the copy in, writes `decide`'s audit lines and tells [`Ctx::on_committed`].
    ///
    /// The last save to finish therefore carries the last state. A caller dropped while it waits
    /// for the gate or the store leaves nothing behind.
    pub async fn transact<T>(
        &self,
        decide: impl FnOnce(&mut Registry) -> Result<(Vec<Audited>, T), Refusal> + Send,
    ) -> Result<Written<T>, Refusal> {
        let _one_at_a_time = self.persist_gate.lock().await;
        let before = self.registry().clone();
        let mut after = before.clone();
        let (lines, value) = decide(&mut after)?;
        if after == before {
            for line in lines {
                self.note(line.app, line.account, line.event);
            }
            return Ok(Written {
                value,
                changed: false,
            });
        }
        self.store
            .save(&after.persisted())
            .await
            .map_err(|_| Refusal::Unavailable)?;
        let listeners: Vec<Told> = self
            .committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let announced = (!listeners.is_empty()).then(|| after.clone());
        *self.registry() = after;
        for line in lines {
            self.note(line.app, line.account, line.event);
        }
        if let Some(after) = announced {
            let committed = Committed { before, after };
            for told in listeners {
                told(committed.clone());
            }
        }
        Ok(Written {
            value,
            changed: true,
        })
    }

    /// Records one audited event, dated now. For an event that is no change of the registry (a
    /// relay opened); a change writes its lines through [`Ctx::transact`].
    pub fn note(&self, app: Option<AppId>, account: Option<AccountId>, event: AuditEvent) {
        self.audit
            .record(AuditEntry::new(self.clock.now(), app, account, event));
    }

    /// Sets an account's state (`NeedsReauth` when its credential was refused, `Ok` again once
    /// it is signed in). Whether it changed; `Unavailable` when the change could not be saved,
    /// and then the state is as it was.
    pub async fn set_state(&self, id: &AccountId, state: AccountState) -> Result<bool, Refusal> {
        let written = self
            .transact(|registry| {
                let changed = registry
                    .accounts
                    .iter_mut()
                    .find(|a| a.id == *id)
                    .is_some_and(|a| std::mem::replace(&mut a.state, state) != state);
                Ok((Vec::new(), changed))
            })
            .await?;
        Ok(written.value)
    }
}
