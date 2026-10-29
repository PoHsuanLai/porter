//! Driving a provider's sign-in through the sheet (porter PLAN G4, G5, §2.4): the host half of
//! the conversation. The sheet's pure machine (`porter_sheet::step`) says what to draw,
//! what to tell the sign-in and when to store; this loop carries those out, and reports what the
//! person did and what the sign-in said back to the machine.
//!
//! While the person is in the browser (or reading a code) the loop polls the sign-in and
//! listens to the sheet at the same time, so closing the sheet ends the wait at once.

use crate::Accounts;
use crate::add_store::AllowFor;
use crate::edit::EditError;
use crate::launcher::{Roster, program_of_spec};
use crate::race::{Raced, race};
use porter_capability::CapabilityKind;
use porter_capability::KindToggle;
use porter_core::ProviderHint;
use porter_core::{AccountId, ProviderId};
use porter_core::{AccountLabel, AppId, AuthKind, SecretText, ServiceEndpoint};
use porter_provider::{
    ConsentHistory, LoginHint, Provider, ProviderError, ProviderSpec, Readiness, SignIn,
    SignInMode, SignInStart, SignInStep, Signed,
};
use porter_service_core::Ports;
use porter_service_core::refusal::refusal_of;
use porter_sheet::host::{SheetFault, SheetLink, SheetOpen, Sheets};
use porter_sheet::{
    EditedServers, FieldAnswer, Manual, Progress, ProviderRow, Purpose, RowKind, Sheet,
    SheetEffect, SheetEnd, SheetEvent, SignInFault, SignInInput, Stage, servers_fixed, step,
};
use porter_wire::AccountsReply;
use porter_wire::{ParentWindow, Refusal};
use std::collections::VecDeque;
use std::future::Future;

/// Who an add is for when the host already knows ("Continue as"): the provider the identity is
/// with, the address its page opens on, and whether this client was signed in for that identity
/// before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInAs {
    /// The provider the identity is with: the sign-in goes straight to it, with no provider list.
    pub provider: ProviderId,
    /// The address to open the provider's page on.
    pub hint: LoginHint,
    /// Whether the person agreed to this client for this identity before. `SignedInBefore` asks
    /// for no consent screen; if the provider then returns nothing to keep, the sign-in is
    /// started once more as `FirstSignIn`.
    pub history: ConsentHistory,
}

impl SignInAs {
    /// A sign-in with `provider` as the person at `hint`, with this `history`.
    pub fn new(provider: ProviderId, hint: LoginHint, history: ConsentHistory) -> Self {
        Self {
            provider,
            hint,
            history,
        }
    }
}

/// What the sheet is for: the part of a conversation that differs between a new account, a
/// sign-in again, one more access and an edit of servers. The conversation around it
/// ([`Accounts::drive`]) is the same for all; what each job stores, and what it ends with
/// (`Done`), is its own, so an entry point holds exactly the result of its own job.
///
/// A default method says "this job cannot": it adds no account, names no identity, keeps no
/// answers.
pub(crate) trait Job: Sync {
    /// What the conversation ends with when it ends well.
    type Done: Send;

    /// What the sheet is opened for, or why it cannot be. `listed` is the provider list as the
    /// sheet would show it.
    fn purpose<E: Ports>(
        &self,
        at: &Accounts<'_, E>,
        caller: &AppId,
        listed: &[ProviderRow],
    ) -> Result<Purpose, Refusal>;

    /// How the provider's sign-in is begun.
    fn mode(&self) -> SignInMode;

    /// Whether the sheet lists the providers to pick from.
    fn lists_providers(&self) -> bool {
        false
    }

    /// Whether this job makes a new account.
    fn adds(&self) -> bool {
        false
    }

    /// Who the sign-in is for and with which consent, when the host already knows.
    fn identity(&self) -> Option<&SignInAs> {
        None
    }

    /// The address to open the provider's page on, when the account says it.
    fn hint(&self) -> Option<LoginHint> {
        None
    }

    /// Whether the store step needs the answers the person typed.
    fn wants_answers(&self) -> bool {
        false
    }

    /// What the conversation ends with when the sign-in showed `held`, an account that was here
    /// already; `None` for a job that makes no account.
    fn there(held: AccountId) -> Option<Self::Done> {
        let _ = held;
        None
    }

    /// Stores what the sign-in produced, as this job says. `provider` is the one the sheet is
    /// working on; `answers` are the last the person sent, when `wants_answers`.
    fn store<E: Ports>(
        &self,
        at: &Accounts<'_, E>,
        caller: &AppId,
        provider: Option<&ProviderId>,
        signed: &Signed,
        answers: Option<&[FieldAnswer]>,
        choices: &[KindToggle],
    ) -> impl Future<Output = Result<Self::Done, ()>> + Send;
}

/// A new account.
pub(crate) struct Add {
    pub(crate) hint: ProviderHint,
    pub(crate) allow: Option<AllowFor>,
    pub(crate) identity: Option<SignInAs>,
}

/// How an add ended well.
pub(crate) enum AddDone {
    Added(AccountId),
    /// Nothing stored: the sign-in showed this account, which was already here.
    AlreadyThere(AccountId),
}

impl Job for Add {
    type Done = AddDone;

    fn purpose<E: Ports>(
        &self,
        at: &Accounts<'_, E>,
        caller: &AppId,
        listed: &[ProviderRow],
    ) -> Result<Purpose, Refusal> {
        if let ProviderHint::Provider(id) = &self.hint
            && !listed.iter().any(|row| row.id == *id)
        {
            // An agent asked for by name that has no launcher is told so.
            let agent = at.ctx.provider_of(id).map(Provider::spec);
            return Err(match agent {
                Some(spec) if launcherless(spec, at.state.roster.get()) => Refusal::NoLauncher,
                _ => Refusal::Unavailable,
            });
        }
        Ok(Purpose::Add {
            hint: self.hint.clone(),
            allow: self.allow.as_ref().map(|_| caller.clone()),
        })
    }

    fn mode(&self) -> SignInMode {
        SignInMode::Add
    }

    fn lists_providers(&self) -> bool {
        true
    }

    fn adds(&self) -> bool {
        true
    }

    fn identity(&self) -> Option<&SignInAs> {
        self.identity.as_ref()
    }

    fn there(held: AccountId) -> Option<AddDone> {
        Some(AddDone::AlreadyThere(held))
    }

    async fn store<E: Ports>(
        &self,
        at: &Accounts<'_, E>,
        caller: &AppId,
        provider: Option<&ProviderId>,
        signed: &Signed,
        _answers: Option<&[FieldAnswer]>,
        choices: &[KindToggle],
    ) -> Result<AddDone, ()> {
        let spec = provider
            .and_then(|id| at.ctx.provider_of(id))
            .map(|provider| provider.spec().clone())
            .ok_or(())?;
        at.store_new(caller, &spec, signed, choices, self.allow.as_ref())
            .await
            .map(AddDone::Added)
    }
}

/// The credential of an existing account again.
pub(crate) struct Reauthenticate {
    pub(crate) account: AccountId,
    pub(crate) provider: ProviderId,
    /// The account's servers as stored, for the family to start from.
    pub(crate) endpoints: Vec<ServiceEndpoint>,
}

impl Job for Reauthenticate {
    type Done = ();

    fn purpose<E: Ports>(
        &self,
        _at: &Accounts<'_, E>,
        _caller: &AppId,
        _listed: &[ProviderRow],
    ) -> Result<Purpose, Refusal> {
        Ok(Purpose::Reauthenticate {
            account: self.account.clone(),
            provider: self.provider.clone(),
        })
    }

    fn mode(&self) -> SignInMode {
        SignInMode::Reauthenticate {
            account: self.account.clone(),
            endpoints: self.endpoints.clone(),
        }
    }

    async fn store<E: Ports>(
        &self,
        at: &Accounts<'_, E>,
        caller: &AppId,
        _provider: Option<&ProviderId>,
        signed: &Signed,
        _answers: Option<&[FieldAnswer]>,
        _choices: &[KindToggle],
    ) -> Result<(), ()> {
        at.store_renewed(caller, &self.account, signed).await
    }
}

/// The on-use access of `kinds` for an existing account, and a credential that holds it.
pub(crate) struct Extend {
    pub(crate) account: AccountId,
    pub(crate) provider: ProviderId,
    /// The account's servers as stored: the family starts from them, and the answer must be
    /// their login.
    pub(crate) endpoints: Vec<ServiceEndpoint>,
    pub(crate) kinds: Vec<CapabilityKind>,
}

impl Job for Extend {
    type Done = ();

    fn purpose<E: Ports>(
        &self,
        _at: &Accounts<'_, E>,
        _caller: &AppId,
        _listed: &[ProviderRow],
    ) -> Result<Purpose, Refusal> {
        Ok(Purpose::Extend {
            account: self.account.clone(),
            provider: self.provider.clone(),
        })
    }

    fn mode(&self) -> SignInMode {
        SignInMode::Extend {
            account: self.account.clone(),
            endpoints: self.endpoints.clone(),
            kinds: self.kinds.clone(),
        }
    }

    /// The page opens on the account's own address: the answer must be that person.
    fn hint(&self) -> Option<LoginHint> {
        login_hint_of(&self.endpoints)
    }

    async fn store<E: Ports>(
        &self,
        at: &Accounts<'_, E>,
        caller: &AppId,
        _provider: Option<&ProviderId>,
        signed: &Signed,
        _answers: Option<&[FieldAnswer]>,
        _choices: &[KindToggle],
    ) -> Result<(), ()> {
        let given: Vec<porter_capability::Capability> = at
            .ctx
            .catalog()
            .get(&self.provider)
            .map(|spec| {
                self.kinds
                    .iter()
                    .flat_map(|kind| spec.on_use_rows(*kind))
                    .map(|row| row.capability.clone())
                    .collect()
            })
            .unwrap_or_default();
        at.store_extended(caller, &self.account, signed, &given)
            .await
    }
}

/// An existing password account's servers, tried at the typed hosts and handed back: nothing is
/// stored here, the step that stores is the one that returns what was checked.
pub(crate) struct EditServers {
    pub(crate) account: AccountId,
    pub(crate) provider: ProviderId,
    pub(crate) auth: AuthKind,
    pub(crate) label: AccountLabel,
    pub(crate) current: Manual,
    pub(crate) name: Option<String>,
    pub(crate) keep: Option<SecretText>,
}

impl Job for EditServers {
    type Done = EditedServers;

    fn purpose<E: Ports>(
        &self,
        _at: &Accounts<'_, E>,
        _caller: &AppId,
        _listed: &[ProviderRow],
    ) -> Result<Purpose, Refusal> {
        Ok(Purpose::EditServers {
            account: self.account.clone(),
            provider: self.provider.clone(),
            auth: self.auth,
            current: self.current.clone(),
            name: self.name.clone(),
        })
    }

    fn mode(&self) -> SignInMode {
        SignInMode::EditServers {
            account: self.account.clone(),
            auth: self.auth,
            label: self.label.clone(),
            current: self.current.clone(),
            name: self.name.clone(),
            keep: self.keep.clone(),
        }
    }

    fn wants_answers(&self) -> bool {
        true
    }

    async fn store<E: Ports>(
        &self,
        _at: &Accounts<'_, E>,
        _caller: &AppId,
        _provider: Option<&ProviderId>,
        signed: &Signed,
        answers: Option<&[FieldAnswer]>,
        _choices: &[KindToggle],
    ) -> Result<EditedServers, ()> {
        let answers = answers.ok_or(())?;
        EditedServers::from_answers(answers, signed.endpoints.clone()).map_err(|_| ())
    }
}

/// A sign-in that fails before it starts or answers.
fn fault_of(error: ProviderError) -> SignInFault {
    match error {
        ProviderError::Unauthorized => SignInFault::Refused,
        ProviderError::Forbidden => SignInFault::Forbidden,
        ProviderError::Unreachable => SignInFault::Unreachable,
        ProviderError::Unreadable => SignInFault::Unreadable,
        // a variant a newer porter adds: the sign-in ends as unreadable
        _ => SignInFault::Unreadable,
    }
}

/// The provider list as the person reads it: the named providers by label (case does not count),
/// then the generic "Other..." rows, mail first. The label is what is drawn, so it is what orders;
/// the provider id (which sorted "Other ACP agent" before "Google") does not.
pub(crate) fn ordered(mut rows: Vec<ProviderRow>) -> Vec<ProviderRow> {
    rows.sort_by_cached_key(|row| {
        (
            row.kind == RowKind::Generic,
            row.id.as_str() != "generic-imap",
            row.label.to_lowercase(),
            row.id.clone(),
        )
    });
    rows
}

/// Providers a person can sign in to: not the ones with nothing to sign in.
fn signable(spec: &ProviderSpec) -> bool {
    !matches!(spec.auth.kind, AuthKind::None | AuthKind::LocalRuntime) && !unlisted(spec)
}

/// Whether the add list shows `provider` now: one a person can sign in to, and whose sign-in
/// can start (an OAuth issuer with a client for this build; an agent whose program has a
/// launcher, when the host keeps a roster). Asked each time a sheet opens, so a client set in
/// Settings, or a launcher that registers, shows its row at the next one.
fn listed<P: Provider>(provider: &P, roster: Option<&Roster>) -> bool {
    signable(provider.spec())
        && provider.readiness() == Readiness::Ready
        && !launcherless(provider.spec(), roster)
}

/// An agent provider whose program no launcher has registered for, by a roster the host keeps:
/// only the agent signs itself in, and with no launcher there is nobody to ask it.
fn launcherless(spec: &ProviderSpec, roster: Option<&Roster>) -> bool {
    match roster {
        Some(roster) => {
            spec.auth.kind == AuthKind::AgentLogin
                && program_of_spec(spec).is_some_and(|program| !roster.has(&program))
        }
        None => false,
    }
}

/// Providers whose file stays loaded (an account made from it still signs in again) but that the
/// add list no longer shows: "Other email account" reaches a JMAP server too, through the
/// protocol choice of its typed-server form.
fn unlisted(spec: &ProviderSpec) -> bool {
    spec.id.as_str() == "generic-jmap"
}

/// The conversation's own state: the sign-in in flight and what it produced.
struct Run<S, D> {
    signin: Option<S>,
    signed: Option<Signed>,
    /// Whether the sign-in is to be polled while the sheet is idle.
    polling: bool,
    /// Who the sign-in is for and with which consent history: what a retry changes.
    identity: Option<SignInAs>,
    stored: Option<D>,
    /// The last answers the person sent, for a job that stores from them.
    answers: Option<Vec<FieldAnswer>>,
    /// The account the sign-in showed again, when it ended `AlreadyAdded`.
    already: Option<AccountId>,
}

/// The run of a job `J` over provider `E::Provider`.
type RunOf<E, J> = Run<<<E as Ports>::Provider as Provider>::SignIn, <J as Job>::Done>;

impl<E: Ports> Accounts<'_, E> {
    /// Opens the add-account sheet, drives the sign-in through it, and answers `Added` with the
    /// new account's id.
    pub(crate) async fn run_add(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
        allow: Option<AllowFor>,
        identity: Option<SignInAs>,
    ) -> AccountsReply {
        let job = Add {
            hint,
            allow,
            identity,
        };
        match Box::pin(self.drive(caller, window, &job)).await {
            Ok(AddDone::Added(id)) => AccountsReply::Added(id),
            // The app is told which account it is, and may ask for a grant of it.
            Ok(AddDone::AlreadyThere(id)) => AccountsReply::AlreadyAdded(id),
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }

    /// Runs a sign-in for `account` that asks for the on-use access of `kinds` alone, and files
    /// the credential it ends with under the same id: the same person only, or nothing is kept.
    pub(crate) async fn run_extend(
        &self,
        caller: &AppId,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> Result<(), Refusal> {
        let (provider, endpoints) = {
            let registry = self.ctx.lock();
            let row = registry
                .accounts
                .iter()
                .find(|a| a.id == *account)
                .ok_or(Refusal::UnknownGrant)?;
            (row.provider.clone(), row.endpoints.clone())
        };
        let asks = kinds.iter().all(|k| self.asks_on_use(&provider, *k));
        if kinds.is_empty() || !asks {
            return Err(Refusal::Unavailable);
        }
        let job = Extend {
            account: account.clone(),
            provider,
            endpoints,
            kinds,
        };
        Box::pin(self.drive(caller, window, &job)).await
    }

    /// Runs the sign-in again for `account` and replaces its credential. Without `any_account`
    /// `caller` must hold a grant for the account; with it (the sheet host and Settings) any
    /// account is signed in again (accountd decides by role who may).
    pub(crate) async fn run_reauthenticate(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        any_account: bool,
    ) -> AccountsReply {
        let (provider, endpoints) = {
            let registry = self.ctx.lock();
            let held = registry.holds(caller, account);
            let row = registry.accounts.iter().find(|a| a.id == *account);
            match (held || any_account, row) {
                (true, Some(row)) => (row.provider.clone(), row.endpoints.clone()),
                _ => return AccountsReply::Refused(Refusal::UnknownGrant),
            }
        };
        let job = Reauthenticate {
            account: account.clone(),
            provider,
            endpoints,
        };
        match Box::pin(self.drive(caller, window, &job)).await {
            Ok(()) => AccountsReply::Reauthenticated,
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }

    /// Opens the sheet `job` is for and drives its sign-in through it, to what the job stores.
    pub(crate) async fn drive<J: Job>(
        &self,
        caller: &AppId,
        window: ParentWindow,
        job: &J,
    ) -> Result<J::Done, Refusal> {
        let rows = ordered(
            self.ctx
                .providers()
                .iter()
                .filter(|provider| listed(*provider, self.state.roster.get()))
                .map(|provider| provider.spec().sheet_row())
                .collect(),
        );
        let purpose = job.purpose(self, caller, &rows)?;
        let rows = match job.lists_providers() {
            true => rows,
            false => Vec::new(),
        };
        let own = match self.state.own_machines.get() {
            Some(machines) => machines.read().await,
            None => Vec::new(),
        };
        let mut sheet = Sheet::new(purpose, rows).with_own_machines(own);
        let mut link = self
            .ctx
            .sheets()
            .conversation(SheetOpen {
                window,
                view: sheet.view(),
            })
            .await
            .map_err(left)?;
        let mut run: RunOf<E, J> = Run {
            signin: None,
            signed: None,
            polling: false,
            identity: job.identity().cloned(),
            stored: None,
            answers: None,
            already: None,
        };
        let mut queue = VecDeque::new();
        // An edit opens on its form, and its sign-in is begun behind it.
        if matches!(sheet.stage, Stage::Working(_))
            || matches!(sheet.purpose, Purpose::EditServers { .. })
        {
            queue.push_back(SheetEffect::Feed(SignInInput::Start));
        }
        loop {
            while let Some(effect) = queue.pop_front() {
                let event = match effect {
                    SheetEffect::Show(view) => {
                        link.update(view).await.map_err(left)?;
                        None
                    }
                    SheetEffect::Feed(input) => self.feed(&sheet, job, &mut run, input).await,
                    SheetEffect::Store(choices) => {
                        Some(self.store(caller, job, &sheet, &mut run, &choices).await)
                    }
                    // The host opens a page it is shown; showing the same page again opens it
                    // again, through the host's own portal path.
                    SheetEffect::OpenBrowser(_) => {
                        link.update(sheet.view()).await.map_err(left)?;
                        None
                    }
                    SheetEffect::Close(end) => {
                        return finish::<J>(end, run.stored.take(), run.already.take());
                    }
                };
                if let Some(event) = event {
                    let (next, effects) = step(sheet, event);
                    sheet = next;
                    queue.extend(effects);
                }
            }
            let event = self.wait(&mut link, &sheet, job, &mut run).await;
            let (next, effects) = step(sheet, event);
            sheet = next;
            queue.extend(effects);
        }
    }

    /// The next thing to tell the machine: what the person did or, while the sign-in is being
    /// polled, what the poll found, whichever comes first.
    async fn wait<J: Job, L: SheetLink>(
        &self,
        link: &mut L,
        sheet: &Sheet,
        job: &J,
        run: &mut RunOf<E, J>,
    ) -> SheetEvent {
        let polled = match (run.polling, run.signin.as_mut()) {
            (true, Some(signin)) => {
                match race(link.input(), signin.next(SignInInput::Poll)).await {
                    Raced::First(input) => return person(input),
                    Raced::Second(step) => step,
                }
            }
            _ => return person(link.input().await),
        };
        run.polling = false;
        self.settled(sheet, job, run, polled).await
    }

    /// What a sign-in's step is to the machine, after the one retry an identity's add owes: a
    /// sign-in that skipped the consent screen (`SignedInBefore`) and ended `Unreadable` (the
    /// provider handed back nothing to keep) starts again once as `FirstSignIn`.
    async fn settled<J: Job>(
        &self,
        sheet: &Sheet,
        job: &J,
        run: &mut RunOf<E, J>,
        step: SignInStep,
    ) -> SheetEvent {
        let retried = match (&step, job.adds(), run.identity.take()) {
            (
                SignInStep::Failed(SignInFault::Unreadable),
                true,
                Some(SignInAs {
                    provider,
                    hint,
                    history: ConsentHistory::SignedInBefore,
                }),
            ) => {
                let again = SignInAs::new(provider, hint, ConsentHistory::FirstSignIn);
                run.identity = Some(again.clone());
                match stage_provider(&sheet.stage).and_then(|id| self.begin(id, job, Some(&again)))
                {
                    Some(Ok(mut signin)) => {
                        let first = signin.next(SignInInput::Start).await;
                        run.signin = Some(signin);
                        Some(first)
                    }
                    Some(Err(_)) | None => None,
                }
            }
            (_, _, identity) => {
                run.identity = identity;
                None
            }
        };
        said(run, retried.unwrap_or(step))
    }

    /// Starts `provider`'s sign-in for `job`, as `identity` when the job is an add for one.
    fn begin<J: Job>(
        &self,
        provider: &ProviderId,
        job: &J,
        identity: Option<&SignInAs>,
    ) -> Option<Result<<E::Provider as Provider>::SignIn, ProviderError>> {
        self.ctx.provider_of(provider).map(|provider| {
            let start = SignInStart::new(job.mode());
            provider.sign_in(match (identity, job.hint()) {
                (Some(SignInAs { hint, history, .. }), _) => {
                    start.with_login_hint(hint.clone()).with_consent(*history)
                }
                (None, Some(hint)) => start.with_login_hint(hint),
                (None, None) => start,
            })
        })
    }

    /// Hands the sign-in what the machine asked it to be told.
    async fn feed<J: Job>(
        &self,
        sheet: &Sheet,
        job: &J,
        run: &mut RunOf<E, J>,
        input: SignInInput,
    ) -> Option<SheetEvent> {
        match input {
            SignInInput::Poll => {
                run.polling = true;
                None
            }
            SignInInput::Cancel => {
                run.polling = false;
                if let Some(mut signin) = run.signin.take() {
                    // The sheet is closing either way: how the sign-in took it is not asked.
                    let _ = signin.next(SignInInput::Cancel).await;
                }
                None
            }
            SignInInput::Start => {
                run.polling = false;
                let id = stage_provider(&sheet.stage)?;
                // An agent signs itself in through its launcher: with none for its program there
                // is nobody to do it, and no account is made to wait for one. The list leaves
                // such an agent out; this is for a launcher that left after the list was drawn.
                if job.adds()
                    && let Some(spec) = self.ctx.provider_of(id).map(Provider::spec)
                    && launcherless(spec, self.state.roster.get())
                {
                    return Some(SheetEvent::SignIn(Progress::Failed(
                        SignInFault::NoLauncher,
                    )));
                }
                let started = self.begin(id, job, run.identity.as_ref());
                let mut signin = match started {
                    Some(Ok(signin)) => signin,
                    Some(Err(error)) => {
                        return Some(SheetEvent::SignIn(Progress::Failed(fault_of(error))));
                    }
                    None => {
                        return Some(SheetEvent::SignIn(Progress::Failed(
                            SignInFault::Unreadable,
                        )));
                    }
                };
                let first = signin.next(SignInInput::Start).await;
                run.signin = Some(signin);
                Some(self.settled(sheet, job, run, first).await)
            }
            other => {
                let signin = run.signin.as_mut()?;
                if job.wants_answers()
                    && let SignInInput::Fields(answers) = &other
                {
                    run.answers = Some(answers.clone());
                }
                let step = signin.next(other).await;
                // The same login twice is not a second account: the sign-in ends here, at the
                // review, and what is stored stays as it is.
                if job.adds()
                    && let Stage::Working(provider) | Stage::Confirming { provider, .. } =
                        &sheet.stage
                    && let Some(held) = self.already_added(provider, &step)
                {
                    run.already = Some(held);
                    if let Some(mut signin) = run.signin.take() {
                        let _ = signin.next(SignInInput::Cancel).await;
                    }
                    return Some(SheetEvent::SignIn(Progress::Failed(
                        SignInFault::AlreadyAdded,
                    )));
                }
                Some(self.settled(sheet, job, run, step).await)
            }
        }
    }

    /// Stores what the sign-in produced, as the job says.
    async fn store<J: Job>(
        &self,
        caller: &AppId,
        job: &J,
        sheet: &Sheet,
        run: &mut RunOf<E, J>,
        choices: &[KindToggle],
    ) -> SheetEvent {
        let Some(signed) = run.signed.take() else {
            return SheetEvent::StoreFailed;
        };
        let answers = run.answers.take();
        // Boxed, like each entry point's call of `drive`: the futures of the four jobs, each
        // generic over the service, are not laid out inside one another.
        let provider = stage_provider(&sheet.stage);
        let stored = Box::pin(job.store(
            self,
            caller,
            provider,
            &signed,
            answers.as_deref(),
            choices,
        ))
        .await;
        match stored {
            Ok(done) => {
                run.stored = Some(done);
                SheetEvent::Stored
            }
            Err(()) => SheetEvent::StoreFailed,
        }
    }
}

/// What a finished sheet answers.
fn finish<J: Job>(
    end: SheetEnd,
    stored: Option<J::Done>,
    already: Option<AccountId>,
) -> Result<J::Done, Refusal> {
    match (end, stored, already) {
        (SheetEnd::Added, Some(stored), _) => Ok(stored),
        (SheetEnd::Added, None, _) => Err(Refusal::Unavailable),
        (SheetEnd::Dismissed, _, _) => Err(Refusal::Dismissed),
        // The account is here already: the app is told which, not that it failed.
        (SheetEnd::Failed(SignInFault::AlreadyAdded), _, Some(held)) => {
            J::there(held).ok_or(Refusal::Unavailable)
        }
        (SheetEnd::Failed(fault), _, _) => Err(refusal_of(fault)),
    }
}

/// What a sheet that could not go on answers: one its host closed or left with is the person
/// leaving, as [`person`] reads it; no host at all is nobody to ask.
fn left(fault: SheetFault) -> Refusal {
    match fault {
        SheetFault::Closed => Refusal::Dismissed,
        _ => Refusal::Unavailable,
    }
}

/// What the person did, as an event; a sheet that closed or went away is the person leaving.
fn person(input: Result<porter_sheet::SheetInput, SheetFault>) -> SheetEvent {
    match input {
        Ok(input) => SheetEvent::Input(input),
        // Closed or unavailable, and a fault a later porter adds: the person is gone.
        Err(_) => SheetEvent::Left,
    }
}

/// The address an account's servers sign in as, as a hint for the provider's page.
fn login_hint_of(endpoints: &[ServiceEndpoint]) -> Option<LoginHint> {
    endpoints
        .iter()
        .find_map(|e| LoginHint::parse(&e.login.0).ok())
}

/// The provider a sign-in under way is for.
fn stage_provider(stage: &Stage) -> Option<&ProviderId> {
    match stage {
        Stage::Working(provider)
        | Stage::Confirming { provider, .. }
        | Stage::Checking { provider, .. }
        | Stage::Asking { provider, .. } => Some(provider),
        _ => None,
    }
}

/// What a sign-in's step is to the machine, keeping the credential here: `Done` leaves the
/// signed account with the run and tells the machine only that it is done.
fn said<S, D>(run: &mut Run<S, D>, step: SignInStep) -> SheetEvent {
    match &step {
        SignInStep::Done(signed) => run.signed = Some(signed.clone()),
        SignInStep::OpenBrowser { .. } | SignInStep::ShowCode { .. } => run.polling = true,
        _ => {}
    }
    SheetEvent::SignIn(step.progress())
}

impl<E: Ports> Accounts<'_, E> {
    /// See [`AccountsApi::asks_on_use`](crate::AccountsApi::asks_on_use).
    pub(crate) fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool {
        self.ctx
            .catalog()
            .get(provider)
            .is_some_and(|spec| spec.on_use_rows(kind).next().is_some())
    }

    /// See [`AccountsApi::edit_servers`](crate::AccountsApi::edit_servers).
    pub(crate) async fn edit_servers(
        &self,
        caller: &AppId,
        edit: porter_sheet::EditServers,
        window: ParentWindow,
    ) -> Result<EditedServers, EditError> {
        let (account, provider, label, current, name, keep) = edit.into_parts();
        // The provider the edit names decides, whether or not this service holds the account: its
        // sign-in must be a typed password, whose servers the person may name.
        let auth = self
            .ctx
            .catalog()
            .get(&provider)
            .map(|spec| spec.auth.kind)
            .ok_or(EditError::Refused(Refusal::Unavailable))?;
        if let Some(fixed) = servers_fixed(auth) {
            return Err(EditError::Fixed(fixed));
        }
        // Where this service does hold the account, the provider named is its own.
        let held = self
            .ctx
            .lock()
            .accounts
            .iter()
            .find(|a| a.id == account)
            .map(|a| a.provider.clone());
        if held.is_some_and(|held| held != provider) {
            return Err(EditError::NotThatProvider);
        }
        let job = EditServers {
            account,
            provider,
            auth,
            label,
            current,
            name,
            keep,
        };
        Box::pin(self.drive(caller, window, &job))
            .await
            .map_err(EditError::Refused)
    }

    /// See [`AccountsApi::add_and_allow`](crate::AccountsApi::add_and_allow).
    pub(crate) async fn add_and_allow(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
        ask: AllowFor,
    ) -> AccountsReply {
        self.run_add(caller, hint, window, Some(ask), None).await
    }
}

#[cfg(test)]
mod tests;
