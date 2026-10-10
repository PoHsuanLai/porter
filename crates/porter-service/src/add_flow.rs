//! Driving a provider's sign-in through the sheet (porter PLAN G4, G5, §2.4): the host half of
//! the conversation. The sheet's pure machine (`porter_core::sheet::step`) says what to draw,
//! what to tell the sign-in and when to store; this loop carries those out, and reports what the
//! person did and what the sign-in said back to the machine.
//!
//! While the person is in the browser (or reading a code) the loop polls the sign-in and
//! listens to the sheet at the same time, so closing the sheet ends the wait at once.

use crate::agent_login::{Roster, program_of_spec};
use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::race::{Raced, race};
use crate::service::AccountService;
use crate::sheets::{SheetFault, SheetLink, SheetOpen, Sheets};
use crate::store::RegistryStore;
use porter_core::consent::Usage;
use porter_core::sheet::{
    Progress, ProviderRow, Purpose, RowKind, Sheet, SheetEffect, SheetEnd, SheetEvent, SignInFault,
    SignInInput, Stage, step,
};
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    AccountId, AccountsReply, AppId, AuthKind, DataClass, Need, ProviderId, ServiceEndpoint,
};
use porter_provider::{
    Provider, ProviderError, ProviderSpec, Readiness, SignIn, SignInMode, SignInStart, SignInStep,
    Signed,
};
use porter_secrets::Secrets;
use std::collections::VecDeque;

/// What an app asked for when no account fit: the account added from its sheet is granted to it
/// in the same step ("Add, and allow Mail to use it"), not by a second prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowFor {
    /// The need the new account must meet to be granted.
    pub need: Need,
    /// The data it touches.
    pub class: DataClass,
    /// Interactive or background.
    pub usage: Usage,
}

/// What the sheet is for.
pub(crate) enum Job {
    /// A new account.
    Add {
        hint: ProviderHint,
        allow: Option<AllowFor>,
    },
    /// The credential of an existing account again.
    Reauthenticate {
        account: AccountId,
        provider: ProviderId,
        /// The account's servers as stored, for the family to start from.
        endpoints: Vec<ServiceEndpoint>,
    },
}

/// How a conversation ended well.
enum Stored {
    Added(AccountId),
    Reauthenticated,
    /// Nothing stored: the sign-in showed this account, which was already here.
    AlreadyThere(AccountId),
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

/// What an app is told when the sheet ends without an account.
pub(crate) fn refusal_of(fault: SignInFault) -> Refusal {
    match fault {
        SignInFault::Cancelled => Refusal::Dismissed,
        SignInFault::NoLauncher => Refusal::NoLauncher,
        SignInFault::Forbidden | SignInFault::Refused => Refusal::Denied,
        SignInFault::Unreachable
        | SignInFault::Unreadable
        | SignInFault::NeedsClientId
        | SignInFault::TimedOut
        | SignInFault::Expired
        | SignInFault::NotInstalled
        | SignInFault::NotRunning
        | SignInFault::SignedOut
        | SignInFault::NotAllowed
        | SignInFault::AlreadyAdded
        | SignInFault::StoreFailed => Refusal::Unavailable,
        // a variant a newer porter adds: the app is told it is unavailable
        _ => Refusal::Unavailable,
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
struct Run<S> {
    signin: Option<S>,
    signed: Option<Signed>,
    /// Whether the sign-in is to be polled while the sheet is idle.
    polling: bool,
    stored: Option<Stored>,
    /// The account the sign-in showed again, when it ended `AlreadyAdded`.
    already: Option<AccountId>,
}

/// Who asks for a sign-in again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reauth {
    /// An app: it needs a grant for the account.
    Held,
    /// The sheet host or Settings: any account, no grant.
    Shell,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Opens the add-account sheet, drives the sign-in through it, and answers `Added` with the
    /// new account's id.
    pub(crate) async fn run_add(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
        allow: Option<AllowFor>,
    ) -> AccountsReply {
        let job = Job::Add { hint, allow };
        match self.drive(caller, window, &job).await {
            Ok(Stored::Added(id)) => AccountsReply::Added(id),
            // The app is told which account it is, and may ask for a grant of it.
            Ok(Stored::AlreadyThere(id)) => AccountsReply::AlreadyAdded(id),
            Ok(Stored::Reauthenticated) => AccountsReply::Refused(Refusal::Unavailable),
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }

    /// Runs the sign-in again for `account` and replaces its credential. `Held` asks that
    /// `caller` hold a grant for the account; `Shell` is for the sheet host and Settings, which
    /// sign any account in again (accountd decides by role who may).
    pub(crate) async fn run_reauthenticate(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        who: Reauth,
    ) -> AccountsReply {
        let (provider, endpoints) = {
            let registry = self.lock();
            let held = registry.grants.iter().any(|g| {
                g.key.app == *caller
                    && g.key.account == *account
                    && g.decision == porter_core::consent::Decision::Allow
            });
            let row = registry.accounts.iter().find(|a| a.id == *account);
            match (held || who == Reauth::Shell, row) {
                (true, Some(row)) => (row.provider.clone(), row.endpoints.clone()),
                _ => return AccountsReply::Refused(Refusal::UnknownGrant),
            }
        };
        let job = Job::Reauthenticate {
            account: account.clone(),
            provider,
            endpoints,
        };
        match self.drive(caller, window, &job).await {
            Ok(Stored::Reauthenticated) => AccountsReply::Reauthenticated,
            Ok(Stored::Added(_) | Stored::AlreadyThere(_)) => {
                AccountsReply::Refused(Refusal::Unavailable)
            }
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }

    fn provider_of(&self, id: &ProviderId) -> Option<&P> {
        self.providers.iter().find(|p| p.spec().id == *id)
    }

    async fn drive(
        &self,
        caller: &AppId,
        window: ParentWindow,
        job: &Job,
    ) -> Result<Stored, Refusal> {
        let rows = ordered(
            self.providers
                .iter()
                .filter(|provider| listed(*provider, self.roster.get()))
                .map(|provider| provider.spec().sheet_row())
                .collect(),
        );
        let purpose = match job {
            Job::Add { hint, allow } => {
                if let ProviderHint::Provider(id) = hint
                    && !rows.iter().any(|row| row.id == *id)
                {
                    // An agent asked for by name that has no launcher is told so.
                    let agent = self.provider_of(id).map(Provider::spec);
                    return Err(match agent {
                        Some(spec) if launcherless(spec, self.roster.get()) => Refusal::NoLauncher,
                        _ => Refusal::Unavailable,
                    });
                }
                Purpose::Add {
                    hint: hint.clone(),
                    allow: allow.as_ref().map(|_| caller.clone()),
                }
            }
            Job::Reauthenticate {
                account, provider, ..
            } => Purpose::Reauthenticate {
                account: account.clone(),
                provider: provider.clone(),
            },
        };
        let rows = match job {
            Job::Add { .. } => rows,
            Job::Reauthenticate { .. } => Vec::new(),
        };
        let mut sheet = Sheet::new(purpose, rows);
        let mut link = self
            .sheets
            .conversation(SheetOpen {
                window,
                view: sheet.view(),
            })
            .await
            .map_err(|_| Refusal::Unavailable)?;
        let mut run = Run {
            signin: None,
            signed: None,
            polling: false,
            stored: None,
            already: None,
        };
        let mut queue = VecDeque::new();
        if matches!(sheet.stage, Stage::Working(_)) {
            queue.push_back(SheetEffect::Feed(SignInInput::Start));
        }
        loop {
            while let Some(effect) = queue.pop_front() {
                let event = match effect {
                    SheetEffect::Show(view) => {
                        link.update(view).await.map_err(|_| Refusal::Unavailable)?;
                        None
                    }
                    SheetEffect::Feed(input) => self.feed(&sheet, job, &mut run, input).await,
                    SheetEffect::Store(choices) => {
                        Some(self.store(caller, job, &sheet, &mut run, &choices).await)
                    }
                    // The host opens a page it is shown; showing the same page again opens it
                    // again, through the host's own portal path.
                    SheetEffect::OpenBrowser(_) => {
                        link.update(sheet.view())
                            .await
                            .map_err(|_| Refusal::Unavailable)?;
                        None
                    }
                    SheetEffect::Close(end) => {
                        return finish(end, run.stored.take(), run.already.take());
                    }
                };
                if let Some(event) = event {
                    let (next, effects) = step(sheet, event);
                    sheet = next;
                    queue.extend(effects);
                }
            }
            let event = self.wait(&mut link, &mut run).await;
            let (next, effects) = step(sheet, event);
            sheet = next;
            queue.extend(effects);
        }
    }

    /// The next thing to tell the machine: what the person did or, while the sign-in is being
    /// polled, what the poll found, whichever comes first.
    async fn wait<L: SheetLink>(&self, link: &mut L, run: &mut Run<P::SignIn>) -> SheetEvent {
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
        said(run, polled)
    }

    /// Hands the sign-in what the machine asked it to be told.
    async fn feed(
        &self,
        sheet: &Sheet,
        job: &Job,
        run: &mut Run<P::SignIn>,
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
                let Stage::Working(id) = &sheet.stage else {
                    return None;
                };
                // An agent signs itself in through its launcher: with none for its program there
                // is nobody to do it, and no account is made to wait for one. The list leaves
                // such an agent out; this is for a launcher that left after the list was drawn.
                if matches!(job, Job::Add { .. })
                    && let Some(spec) = self.provider_of(id).map(Provider::spec)
                    && launcherless(spec, self.roster.get())
                {
                    return Some(SheetEvent::SignIn(Progress::Failed(
                        SignInFault::NoLauncher,
                    )));
                }
                let started = self.provider_of(id).map(|provider| {
                    provider.sign_in(SignInStart::new(match job {
                        Job::Add { .. } => SignInMode::Add,
                        Job::Reauthenticate {
                            account, endpoints, ..
                        } => SignInMode::Reauthenticate {
                            account: account.clone(),
                            endpoints: endpoints.clone(),
                        },
                    }))
                });
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
                Some(said(run, first))
            }
            other => {
                let signin = run.signin.as_mut()?;
                let step = signin.next(other).await;
                // The same login twice is not a second account: the sign-in ends here, at the
                // review, and what is stored stays as it is.
                if let (
                    Job::Add { .. },
                    Stage::Working(provider) | Stage::Confirming { provider, .. },
                ) = (job, &sheet.stage)
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
                Some(said(run, step))
            }
        }
    }

    /// Stores what the sign-in produced, as the job says.
    async fn store(
        &self,
        caller: &AppId,
        job: &Job,
        sheet: &Sheet,
        run: &mut Run<P::SignIn>,
        choices: &[porter_core::sheet::ServiceChoice],
    ) -> SheetEvent {
        let Some(signed) = run.signed.take() else {
            return SheetEvent::StoreFailed;
        };
        let stored = match job {
            Job::Add { allow, .. } => {
                let spec = stage_provider(&sheet.stage)
                    .and_then(|id| self.provider_of(id))
                    .map(|provider| provider.spec().clone());
                match spec {
                    Some(spec) => self
                        .store_new(caller, &spec, &signed, choices, allow.as_ref())
                        .await
                        .map(Stored::Added),
                    None => Err(()),
                }
            }
            Job::Reauthenticate { account, .. } => self
                .store_renewed(caller, account, &signed)
                .await
                .map(|()| Stored::Reauthenticated),
        };
        match stored {
            Ok(stored) => {
                run.stored = Some(stored);
                SheetEvent::Stored
            }
            Err(()) => SheetEvent::StoreFailed,
        }
    }
}

/// What a finished sheet answers.
fn finish(
    end: SheetEnd,
    stored: Option<Stored>,
    already: Option<AccountId>,
) -> Result<Stored, Refusal> {
    match (end, stored, already) {
        (SheetEnd::Added, Some(stored), _) => Ok(stored),
        (SheetEnd::Added, None, _) => Err(Refusal::Unavailable),
        (SheetEnd::Dismissed, _, _) => Err(Refusal::Dismissed),
        // The account is here already: the app is told which, not that it failed.
        (SheetEnd::Failed(SignInFault::AlreadyAdded), _, Some(held)) => {
            Ok(Stored::AlreadyThere(held))
        }
        (SheetEnd::Failed(fault), _, _) => Err(refusal_of(fault)),
    }
}

/// What the person did, as an event; a sheet that closed or went away is the person leaving.
fn person(input: Result<porter_core::sheet::SheetInput, SheetFault>) -> SheetEvent {
    match input {
        Ok(input) => SheetEvent::Input(input),
        Err(SheetFault::Closed | SheetFault::Unavailable) => SheetEvent::Left,
    }
}

/// The provider a sign-in under way is for.
fn stage_provider(stage: &Stage) -> Option<&ProviderId> {
    match stage {
        Stage::Working(provider) | Stage::Confirming { provider, .. } => Some(provider),
        _ => None,
    }
}

/// What a sign-in's step is to the machine, keeping the credential here: `Done` leaves the
/// signed account with the run and tells the machine only that it is done.
fn said<S>(run: &mut Run<S>, step: SignInStep) -> SheetEvent {
    match &step {
        SignInStep::Done(signed) => run.signed = Some(signed.clone()),
        SignInStep::OpenBrowser { .. } | SignInStep::ShowCode { .. } => run.polling = true,
        _ => {}
    }
    SheetEvent::SignIn(step.progress())
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
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
        self.run_add(caller, hint, window, Some(ask)).await
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// The answer "Add Account…" to a consent alert: runs the add sheet with the app's ask
    /// attached, so the account added is granted to it in the same step (one grant, no second
    /// prompt, audited by the add). A cancelled or failed add stores no grant and answers as the
    /// add did; an account added that does not meet the need is kept, ungranted, and the app is
    /// told no account fits.
    pub(crate) async fn add_and_allow_for(
        &self,
        caller: &AppId,
        need: Need,
        asker: crate::registry::Asker<'_>,
        window: &ParentWindow,
    ) -> AccountsReply {
        let ask = AllowFor {
            need: need.clone(),
            class: asker.class,
            usage: asker.usage,
        };
        match self
            .add_and_allow(caller, ProviderHint::Any, window.clone(), ask)
            .await
        {
            AccountsReply::Added(account) => self
                .lock()
                .candidates(&need, asker)
                .into_iter()
                .find(|c| c.account == account)
                .map_or(AccountsReply::Refused(Refusal::NoFittingAccount), |c| {
                    AccountsReply::Chosen(c)
                }),
            // Not an answer to a chooser: the app asks again and is offered the account.
            AccountsReply::AlreadyAdded(_) => AccountsReply::Refused(Refusal::Unavailable),
            refused => refused,
        }
    }
}

#[cfg(test)]
mod tests;
