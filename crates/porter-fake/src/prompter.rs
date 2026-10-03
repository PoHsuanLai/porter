//! A consent sheet with answers written in advance, which remembers what it was asked.

use porter_core::consent::{ConsentAnswer, ConsentAsk, GrantScope};
use porter_core::wire::ParentWindow;
use porter_service::Prompter;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// The sheet stays open: the ask never answers, and the log counts it as abandoned when
    /// its future is dropped (the caller closed the sheet or left).
    Hang,
}

/// Answers asks in order; with the script spent, it dismisses.
#[derive(Debug, Default)]
pub struct ScriptedPrompter {
    script: Mutex<VecDeque<Scripted>>,
    asked: AskLog,
}

/// What a prompter was asked, readable after the prompter moved into a service.
#[derive(Debug, Clone, Default)]
pub struct AskLog {
    asked: Arc<Mutex<Vec<ConsentAsk>>>,
    abandoned: Arc<AtomicUsize>,
}

impl AskLog {
    /// Every ask so far, in order.
    pub fn asked(&self) -> Vec<ConsentAsk> {
        lock(&self.asked).clone()
    }

    /// How many asks were dropped while they waited (a `Hang` that was taken down).
    pub fn abandoned(&self) -> usize {
        self.abandoned.load(Ordering::SeqCst)
    }
}

/// Counts an ask that is dropped before it answers.
struct Abandoned(Arc<AtomicUsize>);

impl Drop for Abandoned {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
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
        lock(&self.asked.asked).push(ask);
        let next = lock(&self.script).pop_front();
        if next == Some(Scripted::Hang) {
            let _abandoned = Abandoned(Arc::clone(&self.asked.abandoned));
            std::future::pending::<()>().await;
        }
        match (next, first) {
            (Some(Scripted::AllowFirst(scope)), Some(account)) => {
                ConsentAnswer::Allow { account, scope }
            }
            (Some(Scripted::Deny), _) => ConsentAnswer::Deny,
            _ => ConsentAnswer::Dismissed,
        }
    }
}
