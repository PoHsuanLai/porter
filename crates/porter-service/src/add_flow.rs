//! Driving a provider's sign-in through the sheet (porter PLAN G4, G5, §2.4): the host half of
//! the conversation. The sheet's pure machine (`porter_core::sheet::step`) says what to draw,
//! what to tell the sign-in and when to store; this loop carries those out, and reports what the
//! person did and what the sign-in said back to the machine.
//!
//! While the person is in the browser (or reading a code) the loop polls the sign-in and
//! listens to the sheet at the same time, so closing the sheet ends the wait at once.

use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::race::{Raced, race};
use crate::service::AccountService;
use crate::sheets::{SheetFault, SheetLink, SheetOpen, Sheets};
use crate::store::RegistryStore;
use porter_core::consent::Usage;
use porter_core::sheet::{
    Progress, Purpose, Sheet, SheetEffect, SheetEnd, SheetEvent, SignInFault, SignInInput, Stage,
    step,
};
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    AccountId, AccountsReply, AppId, AuthKind, DataClass, Need, ProviderId, ServiceEndpoint,
};
use porter_provider::{
    Provider, ProviderError, ProviderSpec, SignIn, SignInMode, SignInStart, SignInStep, Signed,
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
}

/// A sign-in that fails before it starts or answers.
fn fault_of(error: ProviderError) -> SignInFault {
    match error {
        ProviderError::Unauthorized => SignInFault::Refused,
        ProviderError::Forbidden => SignInFault::Forbidden,
        ProviderError::Unreachable => SignInFault::Unreachable,
        ProviderError::Unreadable => SignInFault::Unreadable,
    }
}

/// What an app is told when the sheet ends without an account.
fn refusal_of(fault: SignInFault) -> Refusal {
    match fault {
        SignInFault::Cancelled => Refusal::Dismissed,
        SignInFault::Forbidden | SignInFault::Refused => Refusal::Denied,
        SignInFault::Unreachable
        | SignInFault::Unreadable
        | SignInFault::NeedsClientId
        | SignInFault::TimedOut
        | SignInFault::StoreFailed => Refusal::Unavailable,
    }
}

/// Providers a person can sign in to: not the ones with nothing to sign in.
fn signable(spec: &ProviderSpec) -> bool {
    !matches!(spec.auth.kind, AuthKind::None | AuthKind::LocalRuntime)
}

/// The conversation's own state: the sign-in in flight and what it produced.
struct Run<S> {
    signin: Option<S>,
    signed: Option<Signed>,
    /// Whether the sign-in is to be polled while the sheet is idle.
    polling: bool,
    stored: Option<Stored>,
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
            Ok(Stored::Reauthenticated) => AccountsReply::Refused(Refusal::Unavailable),
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }

    /// Runs the sign-in again for `account` and replaces its credential.
    pub(crate) async fn run_reauthenticate(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> AccountsReply {
        let (provider, endpoints) = {
            let registry = self.lock();
            let held = registry.grants.iter().any(|g| {
                g.key.app == *caller
                    && g.key.account == *account
                    && g.decision == porter_core::consent::Decision::Allow
            });
            let row = registry.accounts.iter().find(|a| a.id == *account);
            match (held, row) {
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
            Ok(Stored::Added(_)) => AccountsReply::Refused(Refusal::Unavailable),
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
        let rows: Vec<_> = self
            .providers
            .iter()
            .map(Provider::spec)
            .filter(|spec| signable(spec))
            .map(ProviderSpec::sheet_row)
            .collect();
        let purpose = match job {
            Job::Add { hint, allow } => {
                if let ProviderHint::Provider(id) = hint
                    && !rows.iter().any(|row| row.id == *id)
                {
                    return Err(Refusal::Unavailable);
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
                    SheetEffect::Close(end) => return finish(end, run.stored.take()),
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
                let started = self.provider_of(id).map(|provider| {
                    provider.sign_in(SignInStart {
                        mode: match job {
                            Job::Add { .. } => SignInMode::Add,
                            Job::Reauthenticate {
                                account, endpoints, ..
                            } => SignInMode::Reauthenticate {
                                account: account.clone(),
                                endpoints: endpoints.clone(),
                            },
                        },
                    })
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
fn finish(end: SheetEnd, stored: Option<Stored>) -> Result<Stored, Refusal> {
    match (end, stored) {
        (SheetEnd::Added, Some(stored)) => Ok(stored),
        (SheetEnd::Added, None) => Err(Refusal::Unavailable),
        (SheetEnd::Dismissed, _) => Err(Refusal::Dismissed),
        (SheetEnd::Failed(fault), _) => Err(refusal_of(fault)),
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
