//! The service: one request in, one reply out, for a caller the transport has identified. The
//! facade a host holds; what it does is the domain modules', over the shared [`Ctx`].

use crate::add_flow::SignInAs;
use crate::add_store::AllowFor;
use crate::agent::AgentFault;
use crate::audit::{AuditEvent, AuditSink, NoAudit};
use crate::ctx::Ctx;
use crate::launcher::{Launchers, Roster};
use crate::local::LocalFault;
use crate::own_machines::OwnMachines;
use crate::registry::Registry;
use crate::sheets::Sheets;
use porter_capability::{CapabilityKind, Claim, Toggle};
use porter_core::LauncherSession;
use porter_core::ProviderHint;
use porter_core::consent::Usage;
use porter_core::store::{NoStore, RegistryStore};
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{AccountState, AgentState, AppId, DataClass, EndpointUrl, RelayPlan, SecretText};
use porter_ids::clock::Clock;
use porter_match::Need;
use porter_provider::{Provider, ProviderSpec, RevokeOutcome};
use porter_secrets::{Secrets, SecretsError};
use porter_wire::{AccountsReply, AccountsRequest};
use porter_wire::{ParentWindow, Refusal};

/// accountd's core over its seams. The store and the audit sink default to none: the registry
/// then lives for the process, and nothing is logged.
#[derive(Debug)]
pub struct AccountService<P, S, U, K, R = NoStore, A = NoAudit> {
    ctx: Ctx<P, S, U, K, R, A>,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock> AccountService<P, S, U, K> {
    /// A service over these providers, starting from `registry` (loaded by the host).
    pub fn new(providers: Vec<P>, registry: Registry, secrets: S, sheets: U, clock: K) -> Self {
        Self {
            ctx: Ctx::new(providers, registry, secrets, sheets, clock),
        }
    }
}

impl<P, S, U, K, R, A> AccountService<P, S, U, K, R, A> {
    /// The same service saving its registry to `store` after every change.
    pub fn with_store<N: RegistryStore>(self, store: N) -> AccountService<P, S, U, K, N, A> {
        AccountService {
            ctx: self.ctx.with_store(store),
        }
    }

    /// The same service recording its decisions to `audit`.
    pub fn with_audit<N: AuditSink>(self, audit: N) -> AccountService<P, S, U, K, R, N> {
        AccountService {
            ctx: self.ctx.with_audit(audit),
        }
    }

    /// Tells the service where the person's computers on their tailnet are listed, so that an
    /// AI server's address in an add sheet may be plain `http` to one of them. Set once; a
    /// service that is never told accepts plain `http` for this computer only.
    pub fn set_own_machines(&self, machines: OwnMachines) {
        let _ = self.ctx.own_machines.set(machines);
    }

    /// Tells the service which agent programs have a launcher, so that adding an agent account
    /// with none ends in `SignInFault::NoLauncher` before anything is stored. Set once; a
    /// service that is never told (no launcher bus: an app hosting porter in process) adds
    /// agent accounts as it always did.
    pub fn set_launcher_roster(&self, roster: Roster) {
        let _ = self.ctx.roster.set(roster);
    }

    /// The same service knowing the local runtimes (`AuthKind::LocalRuntime`) among `specs`:
    /// they have no family to sign in through, but `Peer.ReportLocal` makes their accounts and
    /// the catalogue lists them. Any other spec is ignored (a provider needs its family).
    #[must_use]
    pub fn with_local_runtimes(mut self, specs: Vec<ProviderSpec>) -> Self {
        self.ctx.add_local_runtimes(specs);
        self
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// A copy of the registry, for the host to persist and for the Settings module.
    pub fn registry(&self) -> Registry {
        self.ctx.lock().clone()
    }

    /// Answers `request` from `caller`. `OpenAuthenticated` carries a descriptor the transport
    /// makes, so it is answered by [`AccountService::open_authenticated`] and is `Unavailable`
    /// here.
    pub async fn handle(&self, caller: &AppId, request: AccountsRequest) -> AccountsReply {
        match request {
            AccountsRequest::Query { need, class, usage } => {
                self.ctx.query(caller, &need, class, usage)
            }
            AccountsRequest::Availability { need, class, usage } => {
                self.ctx.availability_of(caller, &need, class, usage)
            }
            AccountsRequest::Choose {
                need,
                class,
                usage,
                window,
            } => {
                self.ctx
                    .choose(caller, need, (class, usage), &window, None)
                    .await
            }
            AccountsRequest::AddAccount { hint, window } => {
                self.ctx.add_account(caller, hint, window).await
            }
            AccountsRequest::Reauthenticate { account, window } => {
                self.ctx.reauthenticate(caller, &account, window).await
            }
            AccountsRequest::ListGrants => self.ctx.grants_of(caller),
            AccountsRequest::Revoke { grant } => self.ctx.revoke(caller, &grant).await,
            AccountsRequest::IssueToken { grant, audience } => {
                self.ctx.issue_token(caller, &grant, &audience).await
            }
            AccountsRequest::OpenAuthenticated { .. } | AccountsRequest::OpenLinked { .. } => {
                AccountsReply::Refused(Refusal::Unavailable)
            }
            // a variant a newer porter adds: refused as unavailable
            _ => AccountsReply::Refused(Refusal::Unavailable),
        }
    }

    /// The API key filed for `account`: the in-process read for a host that runs the agent
    /// launcher (inferd). It never crosses a process. A key that is missing or unreadable (or
    /// filed as another kind of credential) means signing in again (`NeedsReauth`); a store that
    /// is locked or unreachable is `Unavailable`.
    pub async fn api_key(&self, account: &AccountId) -> Result<SecretText, Refusal> {
        self.ctx.api_key(account).await
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
        self.ctx.sign_in_as(caller, identity, window).await
    }

    /// Runs the sign-in again for any account, with no grant: for the sheet host and Settings.
    pub async fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> AccountsReply {
        self.ctx.reauthenticate_any(caller, account, window).await
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
        self.ctx.extend_access(caller, account, kinds, window).await
    }

    /// Whether `provider` asks some access of `kind` only when it is first used (`ask =
    /// "on_use"`): what [`AccountService::extend_access`](crate::AccountService::extend_access) can ask an account of it for, and so whether a
    /// host offers the switch that asks it ("Show in Files").
    pub fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool {
        self.ctx.asks_on_use(provider, kind)
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
        self.ctx.add_and_allow(caller, hint, window, ask).await
    }

    /// Records what the agent says of its own sign-in. Returns whether the account's state
    /// changed (a repeat of the same word changes nothing and writes nothing).
    pub async fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> Result<bool, AgentFault> {
        self.ctx.set_agent_state(account, state).await
    }

    /// Asks for an agent account's login for an app that holds a grant for it.
    pub async fn login_agent(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &impl Launchers,
    ) -> AccountsReply {
        self.ctx
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
        self.ctx
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
        self.ctx.report_local(provider, models, state).await
    }

    /// Removes an account (PLAN §2.9): asks its provider to stop honouring the credential, best
    /// effort, then wipes its secrets, grants, toggles and row, and audits `Removed`. The
    /// provider's answer never stops the wipe. It answers what the provider said, or `None` when
    /// the provider could not be asked (no credential, unreachable, refused).
    pub async fn remove_with_revoke(
        &self,
        id: &AccountId,
    ) -> Result<Option<RevokeOutcome>, SecretsError> {
        self.ctx.remove_with_revoke(id).await
    }

    /// Asks the provider what the account can do now and records it (the claims, with the
    /// account's toggles applied). A refresh token the provider rotated while it looked is read
    /// back from a session opened afterwards (`renewed`) and stored, so the next sign-in
    /// check does not present a token the issuer has since replaced.
    pub async fn rediscover(&self, id: &AccountId) -> Result<Vec<Claim>, Refusal> {
        self.ctx.rediscover(id).await
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
        self.ctx.set_toggle(id, kind, toggle).await
    }

    /// Records `event`, which the host did on the person's behalf (Settings' client id row),
    /// in the service's audit sink.
    pub fn audit_settings(&self, event: AuditEvent) {
        self.ctx.audit_settings(event)
    }

    /// Records `event`, which `app` caused and which concerns no account (a desktop-wide Space
    /// made, renamed or removed), in the service's audit sink.
    pub fn audit_app(&self, app: AppId, event: AuditEvent) {
        self.ctx.audit_app(app, event)
    }

    /// Withdraws any grant, whoever holds it (Settings' "Revoke"). `UnknownGrant` when none has
    /// that id.
    pub async fn revoke_grant(&self, grant: &GrantId) -> Result<(), Refusal> {
        self.ctx.revoke_grant(grant).await
    }

    /// Sets an account's state (`NeedsReauth` when its credential was refused, `Ok` again once
    /// it is signed in). Whether it changed.
    pub async fn set_state(&self, id: &AccountId, state: AccountState) -> bool {
        self.ctx.set_state(id, state).await
    }

    /// Spends `caller`'s grant when it is a `Once` one: the first use of any kind (a token
    /// issued, a relay opened, a key read by the host that holds the secret) drops it, so a
    /// second use asks again. A host that reads a key itself, as inferd does, calls this once
    /// the read has succeeded.
    pub async fn spend_once(&self, caller: &AppId, grant: &GrantId) -> Result<(), Refusal> {
        self.ctx.spend_once(caller, grant).await
    }

    /// Removes the grants scoped to `session`, or every session grant when none is named (a
    /// starting accountd has no open session, so a session grant a file holds is one nobody can
    /// end). Audits each as `SessionGrantEnded`, saves, and returns the grants removed.
    pub async fn end_session_grants(&self, session: Option<&LauncherSession>) -> Vec<GrantId> {
        self.ctx.end_session_grants(session).await
    }

    /// The consent sheet for an agent program's key (`Peer.RequestAgentGrant`): `agent` is the
    /// app `org.quire.Agent.<program>`, the grant is recorded under it, and `session`, which the
    /// caller has checked is open and the launcher's, lets the sheet offer "This session only".
    /// With a session "Add Account…" is not offered a second route: it ends the request
    /// dismissed, since the account it would add and allow gets an `Always` grant, not the
    /// session one.
    pub async fn choose_for_agent(
        &self,
        agent: &AppId,
        need: Need,
        (class, usage): (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&LauncherSession>,
    ) -> AccountsReply {
        self.ctx
            .choose(agent, need, (class, usage), window, session)
            .await
    }

    /// Removes an account and everything filed for it: its provider is asked to stop honouring
    /// the credential first (best effort, `remove_with_revoke` says what it answered), then the
    /// secrets, grants, toggles and the registry row go.
    pub async fn remove_account(&self, id: &AccountId) -> Result<(), SecretsError> {
        self.ctx.remove_with_revoke(id).await.map(|_| ())
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
        self.ctx.open_authenticated(caller, grant, endpoint).await
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
        self.ctx.open_linked(caller, grant, origin).await
    }
}
