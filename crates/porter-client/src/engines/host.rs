//! The [`SessionHost`] for an app that hosts inference itself: it routes `need` -> engine over
//! the table the app gave, under the policy the app gave, and drives the engine's
//! OpenAI-compatible API directly. No inferd, no bus.

use super::engine::{Engine, EngineError};
use super::keys::{KeySource, NoKeys};
use super::routes::{Route, Table};
use super::session::EngineSession;
use super::turn::Pinned;
use crate::error::TransportError;
use crate::transport::SessionHost;
use porter_core::{AppId, DataClass, Need, Tier};
use porter_infer::{InferRefusal, OpenOptions, Policy, Readiness, ServedBy};
use std::sync::Arc;

#[derive(Debug)]
struct Inner<K> {
    table: Table,
    policy: Policy,
    keys: Arc<K>,
}

/// The engines an app points at, the table that says which serves what, the policy that bounds
/// where each class of data may go, and the app's key source. Cheap to clone; every clone is the
/// same host.
#[derive(Debug)]
pub struct EngineHost<K = NoKeys> {
    inner: Arc<Inner<K>>,
}

impl<K> Clone for EngineHost<K> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<K: KeySource + 'static> EngineHost<K> {
    /// A host over `engines`, routed by `routes` (a row per kind of need and tier the app
    /// serves; any other is `Unavailable`), bounded by `policy` (its local-only switch and its
    /// per-class floors; [`Policy::proposed`] is porter's proposal, the app's to take or change),
    /// with `keys` for the engines that want one. Refuses a table that is not one: an engine
    /// given twice, a row naming an engine not given, a key that would cross the network in clear
    /// text.
    pub fn new(
        engines: Vec<Engine>,
        routes: Vec<Route>,
        policy: Policy,
        keys: K,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            inner: Arc::new(Inner {
                table: Table::new(engines, routes)?,
                policy,
                keys: Arc::new(keys),
            }),
        })
    }

    /// The session for `need`, `class` and `tier`: pinned to the engine routed, or refused with
    /// the refusal as its first event.
    async fn session(&self, need: &Need, class: DataClass, tier: Tier) -> EngineSession<K> {
        let inner = &self.inner;
        let chosen = inner
            .table
            .choose(need, class, tier, &inner.policy, &*inner.keys)
            .await;
        let keys = Arc::clone(&inner.keys);
        match chosen {
            Ok(engine) => {
                let served = ServedBy {
                    account: engine.account.clone(),
                    model: engine.model.clone(),
                    locality: engine.locality.clone(),
                };
                let pinned = Pinned {
                    engine: engine.clone(),
                    served,
                    tier,
                };
                EngineSession::pinned(keys, class, pinned)
            }
            Err(refusal) => EngineSession::refused(keys, class, refusal),
        }
    }
}

impl<K: KeySource + 'static> SessionHost for EngineHost<K> {
    type Session = EngineSession<K>;

    async fn open(
        &self,
        _app: &AppId,
        need: &Need,
        class: DataClass,
        tier: Tier,
        _options: &OpenOptions,
    ) -> Result<EngineSession<K>, TransportError> {
        Ok(self.session(need, class, tier).await)
    }

    /// Says what routing says, without touching an engine: `Ready` when an engine is routed (an
    /// engine this host starts nothing on, and does not probe), `Unavailable` when none is, and
    /// any other refusal as `Denied` with its reason.
    async fn prepare(
        &self,
        _app: &AppId,
        need: &Need,
        class: DataClass,
        tier: Tier,
        _options: &OpenOptions,
    ) -> Result<Readiness, TransportError> {
        let inner = &self.inner;
        match inner
            .table
            .choose(need, class, tier, &inner.policy, &*inner.keys)
            .await
        {
            Ok(_) => Ok(Readiness::Ready),
            Err(InferRefusal::Unavailable) => Ok(Readiness::Unavailable),
            Err(refusal) => Err(TransportError::Denied(format!(
                "engine host refused: {refusal}"
            ))),
        }
    }
}
