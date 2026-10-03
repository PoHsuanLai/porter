//! The in-process carrier: the app hosts porter's core itself (mailo standalone, tests). The
//! caller is the app the host names.
//!
//! Accounts are hosted: the service answers every call. Inference is hosted only if the app
//! hands in a broker ([`SessionHost`], through [`InProcess::with_broker`]): porter's own `Broker`
//! is not built, and an app that hosts accounts alone has no inferd, which a session says as it
//! does when inferd is not running: `TransportError::Unreachable`, so a caller degrades as it
//! would on the bus.

use super::Transport;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, AppId, DataClass, Need, Tier};
use porter_infer::{ClientFrame, InferEvent, InferSession, OpenOptions, SessionError};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, Clock, Prompter};
use std::future::Future;
use std::sync::Arc;

/// Where an in-process app's inference sessions come from.
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

/// The core, hosted in this process, answering for one app.
#[derive(Debug)]
pub struct InProcess<P, S, U, K, B = NoBroker> {
    service: Arc<AccountService<P, S, U, K>>,
    app: AppId,
    broker: B,
}

impl<P, S, U, K> InProcess<P, S, U, K> {
    /// `app`'s link to a service this process hosts, with no broker.
    pub fn new(service: Arc<AccountService<P, S, U, K>>, app: AppId) -> Self {
        Self {
            service,
            app,
            broker: NoBroker,
        }
    }
}

impl<P, S, U, K, B> InProcess<P, S, U, K, B> {
    /// The same link with `broker` serving its inference sessions.
    pub fn with_broker<N: SessionHost>(self, broker: N) -> InProcess<P, S, U, K, N> {
        InProcess {
            service: self.service,
            app: self.app,
            broker,
        }
    }
}

impl<P: Provider, S: Secrets, U: Prompter, K: Clock, B: SessionHost> Transport
    for InProcess<P, S, U, K, B>
{
    type Session = B::Session;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        Ok(self.service.handle(&self.app, request).await)
    }

    async fn open_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<B::Session, TransportError> {
        self.broker
            .open(&self.app, need, class, tier, options)
            .await
    }
}
