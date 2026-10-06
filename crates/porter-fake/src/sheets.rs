//! Sheets with answers written in advance, which remember what they were asked and shown: a
//! consent answered from a script, and conversations that read their inputs from one.

use porter_core::consent::{ConsentAnswer, ConsentAsk, GrantScope};
use porter_core::sheet::{SheetInput, SheetView};
use porter_core::wire::ParentWindow;
use porter_service::{SheetFault, SheetLink, SheetOpen, Sheets};
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
    /// "Add Account…".
    AddAccount,
    /// The sheet stays open: the ask never answers, and the log counts it as abandoned when
    /// its future is dropped (the caller closed the sheet or left).
    Hang,
}

/// Answers asks in order (with the script spent, it dismisses) and opens conversations from the
/// inputs written for them (with none left, no host is reachable).
#[derive(Debug, Default)]
pub struct ScriptedSheets {
    script: Mutex<VecDeque<Scripted>>,
    conversations: Mutex<VecDeque<Vec<SheetInput>>>,
    asked: AskLog,
}

/// What sheets were asked and shown, readable after they moved into a service.
#[derive(Debug, Clone, Default)]
pub struct AskLog {
    asked: Arc<Mutex<Vec<ConsentAsk>>>,
    shown: Arc<Mutex<Vec<SheetView>>>,
    abandoned: Arc<AtomicUsize>,
}

impl AskLog {
    /// Every ask so far, in order.
    pub fn asked(&self) -> Vec<ConsentAsk> {
        lock(&self.asked).clone()
    }

    /// Every view a conversation opened with or was updated to, in order.
    pub fn shown(&self) -> Vec<SheetView> {
        lock(&self.shown).clone()
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

impl ScriptedSheets {
    /// Sheets that will answer `script`, in order.
    pub fn answering(script: impl IntoIterator<Item = Scripted>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            ..Self::default()
        }
    }

    /// The same sheets, with one conversation per list: each opens, and its link reads the
    /// inputs of its list in order, then reports the sheet closed.
    pub fn conversing(self, conversations: impl IntoIterator<Item = Vec<SheetInput>>) -> Self {
        Self {
            conversations: Mutex::new(conversations.into_iter().collect()),
            ..self
        }
    }

    /// The log of its asks and views.
    pub fn log(&self) -> AskLog {
        self.asked.clone()
    }
}

/// An open scripted conversation.
#[derive(Debug)]
pub struct ScriptedLink {
    inputs: VecDeque<SheetInput>,
    shown: Arc<Mutex<Vec<SheetView>>>,
}

impl SheetLink for ScriptedLink {
    async fn update(&mut self, view: SheetView) -> Result<(), SheetFault> {
        lock(&self.shown).push(view);
        Ok(())
    }

    async fn input(&mut self) -> Result<SheetInput, SheetFault> {
        self.inputs.pop_front().ok_or(SheetFault::Closed)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while the lock is held already failed the test that caused it.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Sheets for ScriptedSheets {
    type Link = ScriptedLink;

    async fn conversation(&self, open: SheetOpen) -> Result<ScriptedLink, SheetFault> {
        let inputs = lock(&self.conversations)
            .pop_front()
            .ok_or(SheetFault::Unavailable)?;
        lock(&self.asked.shown).push(open.view);
        Ok(ScriptedLink {
            inputs: inputs.into(),
            shown: Arc::clone(&self.asked.shown),
        })
    }

    async fn consent(&self, ask: ConsentAsk, _window: &ParentWindow) -> ConsentAnswer {
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
            (Some(Scripted::AddAccount), _) => ConsentAnswer::AddAccount,
            _ => ConsentAnswer::Dismissed,
        }
    }
}
