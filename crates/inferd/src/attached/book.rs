//! The attached engines of the daemon and what the last look found of each. An attached engine is
//! never started, stopped or evicted: it is in no supervisor, no warm budget and no eviction
//! order, and a stop of the daemon does not reach it. It is looked at when a session opens
//! ([`AttachedBook::reprobe_all`]) and when a model is wanted ([`AttachedBook::reprobe`]), and at
//! no other time.

use super::check::{NotReady, probe};
use crate::local::LocalModel;
use crate::startup::{Cause, Log};
use model_catalog::ModelEntry;
use porter_infer::{ComputerName, ModelRef, Readiness};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

/// What the last look found.
type Outcome = Result<(), NotReady>;

#[derive(Debug, Default)]
struct Inner {
    /// The engines; Settings adds and removes computers while the daemon runs.
    models: RwLock<Vec<Arc<LocalModel>>>,
    /// The names people gave their computers, by the name their place id carries.
    labels: RwLock<BTreeMap<ComputerName, String>>,
    /// The catalogue and the sockets' directory, to make the model of a computer added later.
    catalogue: Vec<ModelEntry>,
    sockets: PathBuf,
    last: RwLock<BTreeMap<ModelRef, Outcome>>,
    log: Log,
}

/// Every attached engine, shared by every clone of the engines.
#[derive(Debug, Clone, Default)]
pub struct AttachedBook(Arc<Inner>);

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

impl AttachedBook {
    /// The book of these models (each made by `attached::local_model`); none has been looked at.
    pub fn new(models: Vec<LocalModel>) -> Self {
        Self(Arc::new(Inner {
            models: RwLock::new(models.into_iter().map(Arc::new).collect()),
            ..Inner::default()
        }))
    }

    /// The same book, its lines logged through `log`.
    pub fn logging_to(self, log: Log) -> Self {
        Self(Arc::new(Inner {
            models: RwLock::new(self.models()),
            labels: RwLock::new(read(&self.0.labels).clone()),
            catalogue: self.0.catalogue.clone(),
            sockets: self.0.sockets.clone(),
            last: RwLock::new(self.last()),
            log,
        }))
    }

    /// The same book that can make the model of a computer added later: over the catalogue
    /// `entries`, with the engines' sockets under `sockets`.
    pub fn with_catalogue(self, entries: Vec<ModelEntry>, sockets: PathBuf) -> Self {
        Self(Arc::new(Inner {
            models: RwLock::new(self.models()),
            labels: RwLock::new(read(&self.0.labels).clone()),
            catalogue: entries,
            sockets,
            last: RwLock::new(self.last()),
            log: self.0.log.clone(),
        }))
    }

    /// The catalogue new models are made over.
    pub fn catalogue(&self) -> &[ModelEntry] {
        &self.0.catalogue
    }

    /// Where the sockets of engines that name none would go.
    pub fn sockets(&self) -> &Path {
        &self.0.sockets
    }

    /// Whether there is nothing attached.
    pub fn is_empty(&self) -> bool {
        read(&self.0.models).is_empty()
    }

    /// Every attached model.
    pub fn models(&self) -> Vec<Arc<LocalModel>> {
        read(&self.0.models).clone()
    }

    /// The attached model `model` names.
    pub fn find(&self, model: &ModelRef) -> Option<Arc<LocalModel>> {
        read(&self.0.models)
            .iter()
            .find(|one| one.model_ref() == *model)
            .cloned()
    }

    /// Takes the models of computer `name` in, under the name the person gave it.
    pub fn add(&self, name: ComputerName, label: String, models: Vec<LocalModel>) {
        write(&self.0.labels).insert(name, label);
        write(&self.0.models).extend(models.into_iter().map(Arc::new));
    }

    /// Drops every model of computer `name`, and what was last found of them.
    pub fn remove_computer(&self, name: &ComputerName) {
        let on = |one: &Arc<LocalModel>| {
            one.attached
                .as_ref()
                .and_then(|target| target.computer.as_ref())
                == Some(name)
        };
        let gone: Vec<ModelRef> = read(&self.0.models)
            .iter()
            .filter(|one| on(one))
            .map(|one| one.model_ref())
            .collect();
        write(&self.0.models).retain(|one| !on(one));
        write(&self.0.labels).remove(name);
        let mut last = write(&self.0.last);
        for model in gone {
            last.remove(&model);
        }
    }

    /// The name the person gave computer `name`, when it was added in Settings.
    pub fn label_of(&self, name: &ComputerName) -> Option<String> {
        read(&self.0.labels).get(name).cloned()
    }

    /// Shows computer `name` under `label` from now on.
    pub fn set_label(&self, name: ComputerName, label: String) {
        write(&self.0.labels).insert(name, label);
    }

    /// The names people gave their computers, for those added in Settings.
    pub fn set_labels(&self, labels: BTreeMap<ComputerName, String>) {
        *write(&self.0.labels) = labels;
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
        let looks = self.models().into_iter().map(|model| {
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
