//! A consent sheet with answers written in advance, which remembers what it was asked.

use porter_core::consent::{ConsentAnswer, ConsentAsk, GrantScope};
use porter_core::wire::ParentWindow;
use porter_service::Prompter;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// One scripted answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scripted {
    /// "Allow" on the first account offered.
    AllowFirst(GrantScope),
    /// "Don't Allow".
    Deny,
    /// The sheet is closed.
    Dismiss,
}

/// Answers asks in order; with the script spent, it dismisses.
#[derive(Debug, Default)]
pub struct ScriptedPrompter {
    script: Mutex<VecDeque<Scripted>>,
    asked: AskLog,
}

/// What a prompter was asked, readable after the prompter moved into a service.
#[derive(Debug, Clone, Default)]
pub struct AskLog(Arc<Mutex<Vec<ConsentAsk>>>);

impl AskLog {
    /// Every ask so far, in order.
    pub fn asked(&self) -> Vec<ConsentAsk> {
        lock(&self.0).clone()
    }
}

impl ScriptedPrompter {
    /// A prompter that will answer `script`, in order.
    pub fn answering(script: impl IntoIterator<Item = Scripted>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            asked: AskLog::default(),
        }
    }

    /// The log of its asks.
    pub fn log(&self) -> AskLog {
        self.asked.clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while the lock is held already failed the test that caused it.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Prompter for ScriptedPrompter {
    async fn ask(&self, ask: ConsentAsk, _window: &ParentWindow) -> ConsentAnswer {
        let first = ask.accounts.first().map(|choice| choice.account.clone());
        lock(&self.asked.0).push(ask);
        let next = lock(&self.script).pop_front();
        match (next, first) {
            (Some(Scripted::AllowFirst(scope)), Some(account)) => {
                ConsentAnswer::Allow { account, scope }
            }
            (Some(Scripted::Deny), _) => ConsentAnswer::Deny,
            _ => ConsentAnswer::Dismissed,
        }
    }
}
