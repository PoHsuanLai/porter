//! Signing an agent in through its launcher (lane agent-login; design/31 R7-R9, C5, C6). Only the
//! agent can log itself in, so there is no sign-in here to drive: accountd asks the launcher that
//! registered the agent's program ([`Launchers`], a seam accountd serves over the bus), the sheet
//! shows `Working` while the launcher's agent does it, and what comes back is a coarse outcome
//! ([`LoginEnd`]) and nothing else. The state change itself (`Ready`) is accountd's, made when the
//! launcher reports, before the sheet hears of it.

use crate::audit::AuditSink;
use crate::ctx::Ctx;
use crate::launcher::{Launchers, LoginEnd, NoLauncher, Waiting, program_of};
use crate::race::{Raced, race};
use crate::sheets::{SheetLink, SheetOpen, Sheets};
use crate::token::refusal_of;
use porter_core::consent::Decision;
use porter_core::sheet::{ProviderRow, SheetInput, SheetView, SignInFault};
use porter_core::store::RegistryStore;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{AccountId, ProviderId};
use porter_core::{AccountsReply, AppId, AuthKind, LoginOutcome};
use porter_ids::clock::Clock;
use porter_provider::Provider;
use porter_secrets::Secrets;

/// Who asks for the login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asker {
    /// An app: it needs a grant for the account.
    Held,
    /// The sheet host or Settings: any account.
    Shell,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    Ctx<P, S, U, K, R, A>
{
    /// The body of [`AccountService::login_agent`](crate::AccountService::login_agent).
    pub(crate) async fn login_agent(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &impl Launchers,
    ) -> AccountsReply {
        self.run_agent_login(caller, account, window, launchers, Asker::Held)
            .await
    }

    /// The body of [`AccountService::login_agent_any`](crate::AccountService::login_agent_any).
    pub(crate) async fn login_agent_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &impl Launchers,
    ) -> AccountsReply {
        self.run_agent_login(caller, account, window, launchers, Asker::Shell)
            .await
    }

    /// The sheet of one login: `Working` while the launcher's agent signs itself in, then the
    /// outcome. A failure waits for the person (`Retry` asks again); closing the sheet ends the
    /// wait without withdrawing the request.
    async fn run_agent_login(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &impl Launchers,
        who: Asker,
    ) -> AccountsReply {
        let (provider, program) = {
            let registry = self.lock();
            let held = registry.grants.iter().any(|g| {
                g.key.app == *caller && g.key.account == *account && g.decision == Decision::Allow
            });
            let row = registry.accounts.iter().find(|a| a.id == *account);
            match (held || who == Asker::Shell, row) {
                (true, Some(row)) if row.auth == AuthKind::AgentLogin => match program_of(row) {
                    Some(program) => (row.provider.clone(), program),
                    None => return AccountsReply::Refused(Refusal::Unavailable),
                },
                (true, Some(_)) => return AccountsReply::Refused(Refusal::Unavailable),
                _ => return AccountsReply::Refused(Refusal::UnknownGrant),
            }
        };
        let row = self.catalog.get(&provider).map(|spec| spec.sheet_row());
        let Ok(mut link) = self
            .sheets
            .conversation(SheetOpen {
                window,
                view: SheetView::Working {
                    provider: provider.clone(),
                    row: row.clone(),
                },
            })
            .await
        else {
            return AccountsReply::Refused(Refusal::Unavailable);
        };
        loop {
            let end = match launchers.ask_login(account, &program).await {
                Err(NoLauncher) => Err(SignInFault::NoLauncher),
                Ok(waiting) => match wait(&mut link, waiting).await {
                    Waited::Left => return AccountsReply::Refused(Refusal::Dismissed),
                    Waited::Ended(LoginEnd::Reported(LoginOutcome::Ready)) => Ok(()),
                    Waited::Ended(end) => Err(end.fault()),
                },
            };
            match end {
                Ok(()) => {
                    // The state is already `Ok`; the sheet says so and goes.
                    let _ = link.update(SheetView::Done).await;
                    return AccountsReply::Reauthenticated;
                }
                Err(fault) => match failed(&mut link, (&provider, &row), fault).await {
                    Next::Retry => continue,
                    Next::Close => return AccountsReply::Refused(refusal_of(fault)),
                },
            }
        }
    }
}

enum Waited {
    /// The person closed the sheet, or it went away.
    Left,
    Ended(LoginEnd),
}

/// Waits for the launcher's answer while the sheet is up. Nothing the person can press on
/// `Working` changes the wait except closing it.
async fn wait<L: SheetLink>(link: &mut L, mut waiting: Waiting) -> Waited {
    loop {
        match race(link.input(), &mut waiting).await {
            Raced::Second(end) => return Waited::Ended(end),
            Raced::First(Ok(SheetInput::Dismiss) | Err(_)) => return Waited::Left,
            Raced::First(Ok(_)) => {}
        }
    }
}

enum Next {
    Retry,
    Close,
}

/// Shows why it failed and waits for the person.
async fn failed<L: SheetLink>(
    link: &mut L,
    (provider, row): (&ProviderId, &Option<ProviderRow>),
    fault: SignInFault,
) -> Next {
    let shown = link
        .update(SheetView::Failed {
            provider: provider.clone(),
            row: row.clone(),
            fault,
        })
        .await;
    match (shown, link.input().await) {
        (Ok(()), Ok(SheetInput::Retry)) => Next::Retry,
        // Updating a closed sheet fails; the person closing it is the same answer.
        _ => Next::Close,
    }
}
