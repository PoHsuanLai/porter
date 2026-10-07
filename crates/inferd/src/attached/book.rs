//! The attached engines of the daemon and what the last look found of each. An attached engine is
//! never started, stopped or evicted: it is in no supervisor, no warm budget and no eviction
//! order, and a stop of the daemon does not reach it. It is looked at when a session opens
//! ([`AttachedBook::reprobe_all`]) and when a model is wanted ([`AttachedBook::reprobe`]), and at
//! no other time.

use super::check::{NotReady, probe};
use crate::local::LocalModel;
use crate::startup::{Cause, Log};
use porter_infer::{ModelRef, Readiness};
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

/// What the last look found.
type Outcome = Result<(), NotReady>;

#[derive(Debug, Default)]
struct Inner {
    models: Vec<Arc<LocalModel>>,
    last: RwLock<BTreeMap<ModelRef, Outcome>>,
    log: Log,
}

/// Every attached engine, shared by every clone of the engines.
#[derive(Debug, Clone, Default)]
pub struct AttachedBook(Arc<Inner>);

impl AttachedBook {
    /// The book of these models (each made by `attached::local_model`); none has been looked at.
    pub fn new(models: Vec<LocalModel>) -> Self {
        Self(Arc::new(Inner {
            models: models.into_iter().map(Arc::new).collect(),
            ..Inner::default()
        }))
    }

    /// The same book, its lines logged through `log`.
    pub fn logging_to(self, log: Log) -> Self {
        Self(Arc::new(Inner {
            models: self.0.models.clone(),
            last: RwLock::new(self.last()),
            log,
        }))
    }

    /// Whether there is nothing attached.
    pub fn is_empty(&self) -> bool {
        self.0.models.is_empty()
    }

    /// Every attached model.
    pub fn models(&self) -> &[Arc<LocalModel>] {
        &self.0.models
    }

    /// The attached model `model` names.
    pub fn find(&self, model: &ModelRef) -> Option<Arc<LocalModel>> {
        self.0
            .models
            .iter()
            .find(|one| one.model_ref() == *model)
            .cloned()
    }

    fn last(&self) -> BTreeMap<ModelRef, Outcome> {
        self.0
            .last
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What the last look found of `model`; none before the first look.
    pub fn state(&self, model: &ModelRef) -> Option<Outcome> {
        self.last().get(model).cloned()
    }

    /// How ready `model` is by the last look: `Ready` when it answered, `Unavailable` when it did
    /// not or has not been looked at (the next open looks).
    pub fn readiness(&self, model: &ModelRef) -> Readiness {
        match self.state(model) {
            Some(Ok(())) => Readiness::Ready,
            _ => Readiness::Unavailable,
        }
    }

    /// Looks at `model` now and keeps what it found. A line is logged when what it found changed
    /// (never the token, and nothing of the data).
    pub async fn reprobe(&self, model: &ModelRef) -> Outcome {
        let Some(local) = self.find(model) else {
            return Err(NotReady::Unanswered);
        };
        let Some(target) = local.attached.as_ref() else {
            return Err(NotReady::Unanswered);
        };
        let found = probe(target, &local.name.0).await;
        let before = self
            .0
            .last
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(model.clone(), found.clone());
        if before.as_ref() != Some(&found) {
            self.say(&local, &found);
        }
        found
    }

    /// Looks at every attached engine, all at once, as a session opens.
    pub async fn reprobe_all(&self) {
        let looks = self.0.models.iter().map(|model| {
            let (book, model) = (self.clone(), model.model_ref());
            tokio::spawn(async move { book.reprobe(&model).await })
        });
        for look in looks.collect::<Vec<_>>() {
            let _ = look.await;
        }
    }

    fn say(&self, local: &LocalModel, found: &Outcome) {
        if let Err(why) = found {
            self.0
                .log
                .failed(&local.spec.id, &Cause::Attached(why.clone()));
        }
    }
}

#[cfg(test)]
mod tests;
