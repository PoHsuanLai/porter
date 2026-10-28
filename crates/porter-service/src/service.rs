//! The service: one request in, one reply out, for a caller the transport has identified. The
//! facade a host holds; what it does is the domains', over the shared [`Ctx`].

use porter_capability::{CapabilityKind, Claim, Toggle};
use porter_core::ProviderHint;
use porter_core::consent::{Availability, Grant, Usage};
use porter_core::store::{NoStore, RegistryStore};
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{
    AccountState, AgentState, AppId, Candidate, DataClass, EndpointUrl, LlmVerdict, OwnServerRow,
    RelayPlan, SecretText, SessionToken,
};
use porter_ids::clock::Clock;
use porter_match::Need;
use porter_provider::{Provider, ProviderSpec, RevokeOutcome};
use porter_secrets::{Secrets, SecretsError};
use porter_service_accounts::{
    Accounts, AccountsApi, AccountsState, AgentFault, AllowFor, Launchers, LocalFault, OwnMachines,
    Roster, SignInAs,
};
use porter_service_core::audit::{AuditEvent, AuditSink, NoAudit};
use porter_service_core::registry::Registry;
use porter_service_core::{Ctx, Parts};
use porter_service_grants::{Grants, GrantsApi};
use porter_sheet::host::Sheets;
use porter_wire::{AccountsReply, AccountsRequest};
use porter_wire::{ParentWindow, Refusal};

/// accountd's core over its seams. The store and the audit sink default to none: the registry
/// then lives for the process, and nothing is logged.
pub struct AccountService<P, S, U, K, R = NoStore, A = NoAudit>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
{
    ctx: Ctx<Parts<P, S, U, K, R, A>>,
    accounts: AccountsState,
}

impl<P, S, U, K, R, A> std::fmt::Debug for AccountService<P, S, U, K, R, A>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountService").finish_non_exhaustive()
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock> AccountService<P, S, U, K> {
    /// A service over these providers, starting from `registry` (loaded by the host).
    pub fn new(providers: Vec<P>, registry: Registry, secrets: S, sheets: U, clock: K) -> Self {
        Self {
            ctx: Ctx::new(providers, registry, secrets, sheets, clock),
            accounts: AccountsState::default(),
        }
    }
}

impl<P, S, U, K, R, A> AccountService<P, S, U, K, R, A>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
{
    /// The same service saving its registry to `store` after every change.
    pub fn with_store<N: RegistryStore>(self, store: N) -> AccountService<P, S, U, K, N, A> {
        AccountService {
            ctx: self.ctx.with_store(store),
            accounts: self.accounts,
        }
    }

    /// The same service recording its decisions to `audit`.
    pub fn with_audit<N: AuditSink>(self, audit: N) -> AccountService<P, S, U, K, R, N> {
        AccountService {
            ctx: self.ctx.with_audit(audit),
            accounts: self.accounts,
        }
    }

    /// Tells the service where the person's computers on their tailnet are listed, so that an
    /// AI server's address in an add sheet may be plain `http` to one of them. Set once; a
    /// service that is never told accepts plain `http` for this computer only.
    pub fn set_own_machines(&self, machines: OwnMachines) {
        self.accounts.set_own_machines(machines);
    }

    /// Tells the service which agent programs have a launcher, so that adding an agent account
    /// with none ends in `SignInFault::NoLauncher` before anything is stored. Set once; a
    /// service that is never told (no launcher bus: an app hosting porter in process) adds
    /// agent accounts as it always did.
    pub fn set_launcher_roster(&self, roster: Roster) {
        self.accounts.set_launcher_roster(roster);
    }

    /// The same service knowing the local runtimes (`AuthKind::LocalRuntime`) among `specs`:
    /// they have no family to sign in through, but `Peer.ReportLocal` makes their accounts and
    /// the catalogue lists them. Any other spec is ignored (a provider needs its family).
    #[must_use]
    pub fn with_local_runtimes(mut self, specs: Vec<ProviderSpec>) -> Self {
        self.ctx.add_local_runtimes(specs);
        self
    }

    fn accounts(&self) -> Accounts<'_, Parts<P, S, U, K, R, A>> {
        Accounts::over(&self.ctx, &self.accounts)
    }

    fn grants_domain(&self) -> Grants<'_, Parts<P, S, U, K, R, A>> {
        Grants::over(&self.ctx, self.accounts())
    }

    /// A copy of the registry, for the host to persist and for the Settings module.
    pub fn registry(&self) -> Registry {
        self.ctx.lock().clone()
    }

    /// Answers `request` from `caller`: a sheet (a chooser, the add-account sheet, a sign-in
    /// again). What an app reads of its grants is [`AccountService::candidates`],
    /// [`AccountService::availability`] and [`AccountService::grants`]; a relay carries a
    /// descriptor the transport makes, so it is [`AccountService::open_authenticated`] and
    /// [`AccountService::open_linked`].
    pub async fn handle(&self, caller: &AppId, request: AccountsRequest) -> AccountsReply {
        match request {
            AccountsRequest::Choose {
                need,
                class,
                usage,
                window,
            } => {
                self.choose(caller, need, (class, usage), &window, None)
                    .await
            }
            AccountsRequest::AddAccount { hint, window } => {
                self.accounts().add_account(caller, hint, window).await
            }
            AccountsRequest::Reauthenticate { account, window } => {
                self.accounts()
                    .reauthenticate(caller, &account, window)
                    .await
            }
            // a variant a newer porter adds: refused as unavailable
            _ => AccountsReply::Refused(Refusal::Unavailable),
        }
    }

    /// The accounts that fit `need` for `caller` and that `caller` holds a grant for, best first.
    pub fn candidates(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Vec<Candidate> {
        self.grants_domain().candidates(caller, need, class, usage)
    }

    /// Whether `need` can be met for `caller`, revealing no identity: an account fits and
    /// `caller` holds a grant, an account fits and the person is yet to be asked, or none fits.
    pub fn availability(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Availability {
        self.grants_domain()
            .availability(caller, need, class, usage)
    }

    /// The grants `caller` holds.
    pub fn grants(&self, caller: &AppId) -> Vec<Grant> {
        self.grants_domain().grants(caller)
    }

    /// Withdraws one of `caller`'s own grants; `UnknownGrant` when `caller` holds none of that
    /// id.
    pub async fn revoke(&self, caller: &AppId, grant: &GrantId) -> Result<(), Refusal> {
        self.grants_domain().revoke(caller, grant).await
    }

    /// Shows the chooser and consent sheet for `need` and records the grant the person gives.
    /// `session` is the open session the request was made in (the agent launcher's, for an agent
    /// program's key: `caller` is then the app `org.quire.Agent.<program>`), which lets the
    /// sheet offer "This session only". With a session "Add Account…" is not offered a second
    /// route: it ends the request dismissed, since the account it would add and allow gets an
    /// `Always` grant, not the session one.
    pub async fn choose(
        &self,
        caller: &AppId,
        need: Need,
        class_usage: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&SessionToken>,
    ) -> AccountsReply {
        self.grants_domain()
            .choose(caller, need, class_usage, window, session)
            .await
    }

    /// The API key filed for `account`: the in-process read for a host that runs the agent
    /// launcher (inferd). It never crosses a process. A key that is missing or unreadable (or
    /// filed as another kind of credential) means signing in again (`NeedsReauth`); a store that
    /// is locked or unreachable is `Unavailable`.
    pub async fn api_key(&self, account: &AccountId) -> Result<SecretText, Refusal> {
        self.accounts().api_key(account).await
    }

    /// The accounts that are servers of the person's own, with their chat addresses.
    pub fn own_servers(&self) -> Vec<OwnServerRow> {
        self.accounts().own_servers()
    }

    /// The key of the own server `account`, which needs no permit.
    pub async fn own_server_key(&self, account: &AccountId) -> Result<SecretText, Refusal> {
        self.accounts().own_server_key(account).await
    }

    /// What each account that serves language models says for `app`.
    pub fn llm_verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
        provider_label: &dyn Fn(&ProviderId) -> Option<String>,
    ) -> Vec<LlmVerdict> {
        self.grants_domain()
            .llm_verdicts(app, class, usage, provider_label)
    }

    /// The key behind an allow grant of kind `Llm`, read once; an allow-once grant is spent.
    pub async fn release_key(&self, grant: &GrantId) -> Result<SecretText, Refusal> {
        self.grants_domain().release_key(grant).await
    }

    /// The permit `app` already holds for `need`, the newest.
    pub fn existing_permit(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Option<Candidate> {
        self.grants_domain()
            .existing_permit(app, need, class, usage)
    }

    /// Opens the add-account sheet for an identity the host already knows ("Continue as"): the
    /// provider's page opens on `identity.hint`, and a sign-in that skipped the consent screen
    /// and came back with nothing to keep is started once more as a first sign-in. The sheet
    /// opens on `identity.provider` with no provider list; no grant is recorded.
    pub async fn sign_in_as(
        &self,
        caller: &AppId,
        identity: SignInAs,
        window: ParentWindow,
    ) -> AccountsReply {
        self.accounts().sign_in_as(caller, identity, window).await
    }

    /// Runs the sign-in again for any account, with no grant: for the sheet host and Settings.
    pub async fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> AccountsReply {
        self.accounts()
            .reauthenticate_any(caller, account, window)
            .await
    }

    /// Asks the person, in a sign-in of its own, for the access `account`'s provider asks only
    /// when it is first used (`ask = "on_use"`) for `kinds`: the whole of a drive, when the
    /// person turns on "Show in Files". The sheet opens on the account, the provider's page asks
    /// for that access alone, and the review names it in plain words; on Allow the new
    /// credential replaces the old under the same id, and the account claims the access given
    /// (the provider's on-use rows of `kinds`), so a need of it fits the account from then on:
    /// an account fits such a need only once the person agreed. An answer from anyone but the account's
    /// own login stores nothing. For the host itself (syncd, for its own accounts): no grant is
    /// asked of `caller`, who is named in the audit.
    ///
    /// # Errors
    /// `UnknownGrant` for an account the registry does not hold; `Unavailable` when its provider
    /// asks none of `kinds` on use, or when what came back could not be kept (another person
    /// included); otherwise the sheet's own ending (`Dismissed`, `Denied` when the person
    /// refused the access).
    pub async fn extend_access(
        &self,
        caller: &AppId,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> Result<(), Refusal> {
        self.accounts()
            .extend_access(caller, account, kinds, window)
            .await
    }

    /// Whether `provider` asks some access of `kind` only when it is first used (`ask =
    /// "on_use"`): what [`AccountService::extend_access`] can ask an account of it for, and so
    /// whether a host offers the switch that asks it ("Show in Files").
    pub fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool {
        self.accounts().asks_on_use(provider, kind)
    }

    /// Opens the add-account sheet as `AddAccount` does and, when it ends in an account that
    /// meets `ask`, records one grant of it for `caller` in the same step: the sheet's last
    /// button reads "Add, and allow". This is what a `Choose` that found no account opens.
    pub async fn add_and_allow(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
        ask: AllowFor,
    ) -> AccountsReply {
        self.accounts()
            .add_and_allow(caller, hint, window, ask)
            .await
    }

    /// Records what the agent says of its own sign-in. Returns whether the account's state
    /// changed (a repeat of the same word changes nothing and writes nothing).
    pub async fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> Result<bool, AgentFault> {
        self.accounts().set_agent_state(account, state).await
    }

    /// Asks for an agent account's login for an app that holds a grant for it.
    pub async fn login_agent(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &impl Launchers,
    ) -> AccountsReply {
        self.accounts()
            .login_agent(caller, account, window, launchers)
            .await
    }

    /// Asks for any agent account's login, with no grant: for the sheet host and Settings.
    pub async fn login_agent_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &impl Launchers,
    ) -> AccountsReply {
        self.accounts()
            .login_agent_any(caller, account, window, launchers)
            .await
    }

    /// Takes what a probe found: the account of `provider` (its id is the provider's, one per
    /// runtime) is made if it is not there, holds `models` as its claims, and is in `state`.
    ///
    /// An `offline` report with no models keeps the models the account had, so a stopped runtime
    /// still lists what it ran and comes back as it was. An `ok` report replaces them.
    pub async fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> Result<AccountId, LocalFault> {
        self.accounts().report_local(provider, models, state).await
    }

    /// Removes an account (PLAN §2.9): asks its provider to stop honouring the credential, best
    /// effort, then wipes its secrets, grants, toggles and row, and audits `Removed`. The
    /// provider's answer never stops the wipe. It answers what the provider said, or `None` when
    /// the provider could not be asked (no credential, unreachable, refused).
    pub async fn remove_with_revoke(
        &self,
        id: &AccountId,
    ) -> Result<Option<RevokeOutcome>, SecretsError> {
        self.accounts().remove_with_revoke(id).await
    }

    /// Asks the provider what the account can do now and records it (the claims, with the
    /// account's toggles applied). A refresh token the provider rotated while it looked is read
    /// back from a session opened afterwards (`renewed`) and stored, so the next sign-in
    /// check does not present a token the issuer has since replaced.
    pub async fn rediscover(&self, id: &AccountId) -> Result<Vec<Claim>, Refusal> {
        self.accounts().rediscover(id).await
    }

    /// Switches one kind of one account on or off: the toggle row is kept, and the account's
    /// effective capabilities follow (a kind turned off reads `Absent { TurnedOff }`; turned on
    /// again it reads as its provider file declares it until the next discovery refreshes it).
    /// A switch that changes the kind is audited (`ServiceToggled`).
    pub async fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> Result<(), Refusal> {
        self.accounts().set_toggle(id, kind, toggle).await
    }

    /// Records `event`, which the host did on the person's behalf (Settings' client id row),
    /// in the service's audit sink.
    pub fn audit_settings(&self, event: AuditEvent) {
        self.ctx.note(None, None, event);
    }

    /// Records `event`, which `app` caused and which concerns no account (a desktop-wide Space
    /// made, renamed or removed), in the service's audit sink.
    pub fn audit_app(&self, app: AppId, event: AuditEvent) {
        self.ctx.note(Some(app), None, event);
    }

    /// Withdraws any grant, whoever holds it (Settings' "Revoke"). `UnknownGrant` when none has
    /// that id.
    pub async fn revoke_grant(&self, grant: &GrantId) -> Result<(), Refusal> {
        self.grants_domain().revoke_grant(grant).await
    }

    /// Sets an account's state (`NeedsReauth` when its credential was refused, `Ok` again once
    /// it is signed in). Whether it changed.
    pub async fn set_state(&self, id: &AccountId, state: AccountState) -> bool {
        self.ctx.set_state(id, state).await
    }

    /// Spends `caller`'s grant when it is a `Once` one: the first use of any kind (a relay
    /// opened, a key read by the host that holds the secret) drops it, so a second use asks
    /// again. A host that reads a key itself, as inferd does, calls this once the read has
    /// succeeded.
    pub async fn spend_once(&self, caller: &AppId, grant: &GrantId) -> Result<(), Refusal> {
        self.grants_domain().spend_once(caller, grant).await
    }

    /// Removes the grants scoped to `session`, or every session grant when none is named (a
    /// starting accountd has no open session, so a session grant a file holds is one nobody can
    /// end). Audits each as `SessionGrantEnded`, saves, and returns the grants removed.
    pub async fn end_session_grants(&self, session: Option<&SessionToken>) -> Vec<GrantId> {
        self.grants_domain().end_session_grants(session).await
    }

    /// Removes an account and everything filed for it: its provider is asked to stop honouring
    /// the credential first (best effort, `remove_with_revoke` says what it answered), then the
    /// secrets, grants, toggles and the registry row go.
    pub async fn remove_account(&self, id: &AccountId) -> Result<(), SecretsError> {
        self.accounts().remove_with_revoke(id).await.map(|_| ())
    }

    /// What the relay for `OpenAuthenticated` is told: checks that `caller` holds the grant and
    /// that `endpoint` is one of the account's for the grant's kind, then reads what the relay
    /// presents. The transport makes the descriptor and runs the relay.
    pub async fn open_authenticated(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        self.grants_domain()
            .open_authenticated(caller, grant, endpoint)
            .await
    }

    /// What the relay for `OpenLinked` is told: checks that `caller` holds the grant and that
    /// `origin` is one its account's provider file declares for the grant's kind, then plans a
    /// relay that adds no credential. The audit line names the origin, never a link's path (a
    /// pre-authenticated URL carries its secret there).
    pub async fn open_linked(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        self.grants_domain()
            .open_linked(caller, grant, origin)
            .await
    }
}
