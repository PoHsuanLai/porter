//! Where inference sessions come from for an app that hosts them itself: the [`SessionHost`] a
//! carrier takes ([`super::Brokered`], or `engines::EngineHost`), and the
//! [`NoBroker`] an app that hosts no inference names.

use crate::error::TransportError;
use porter_capability::Tier;
use porter_core::{AppId, DataClass};
use porter_infer::{ClientFrame, InferEvent, InferSession, OpenOptions, Readiness, SessionError};
use porter_match::Need;
use std::future::Future;
use std::sync::Arc;

/// Where an in-process app's inference sessions come from (feature `infer`).
pub trait SessionHost: Send + Sync {
    /// The session `open` returns.
    type Session: InferSession;

    /// Opens a session for `app`, pinned to the model the host routes `need`, `class` and `tier`
    /// to. A refusal arrives as the session's first event, as on the bus.
    fn open(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> impl Future<Output = Result<Self::Session, TransportError>> + Send;

    /// Warms the engine the host would route `need`, `class` and `tier` to and says how ready it
    /// is. A host that has no engines to warm keeps the default, `Unreachable`.
    fn prepare(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> impl Future<Output = Result<Readiness, TransportError>> + Send {
        let _ = (app, need, class, tier, options);
        async { Err(TransportError::Unreachable) }
    }
}

/// One broker may serve several apps' links.
impl<T: SessionHost> SessionHost for Arc<T> {
    type Session = T::Session;

    fn open(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> impl Future<Output = Result<T::Session, TransportError>> + Send {
        (**self).open(app, need, class, tier, options)
    }

    fn prepare(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> impl Future<Output = Result<Readiness, TransportError>> + Send {
        (**self).prepare(app, need, class, tier, options)
    }
}

/// No broker hosted: every `open` is `Unreachable`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoBroker;

/// The session of [`NoBroker`]: there is none, so this holds nothing and is never made.
#[derive(Debug)]
pub struct InProcessSession(Never);

#[derive(Debug)]
enum Never {}

impl InferSession for InProcessSession {
    async fn send(&mut self, _frame: ClientFrame) -> Result<(), SessionError> {
        match self.0 {}
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        match self.0 {}
    }
}

impl SessionHost for NoBroker {
    type Session = InProcessSession;

    async fn open(
        &self,
        _app: &AppId,
        _need: &Need,
        _class: DataClass,
        _tier: Tier,
        _options: &OpenOptions,
    ) -> Result<InProcessSession, TransportError> {
        Err(TransportError::Unreachable)
    }
}
