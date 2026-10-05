//! The in-process carrier: the app hosts porter's core itself (mailo standalone, tests). The
//! caller is the app the host names.
//!
//! Accounts are hosted: the service answers every call. Inference is hosted only if the app
//! hands in a broker ([`SessionHost`], through [`InProcess::with_broker`]): porter's own `Broker`
//! is not built, and an app that hosts accounts alone has no inferd, which a session says as it
//! does when inferd is not running: `TransportError::Unreachable`, so a caller degrades as it
//! would on the bus.

use super::Transport;
use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::error::TransportError;
use crate::relays::{NoRelays, RelayHost};
use porter_core::stream::duplex;
use porter_core::{
    AccountsReply, AccountsRequest, AppId, DataClass, EndpointUrl, GrantId, Need, Tier,
};
use porter_infer::{ClientFrame, InferEvent, InferSession, OpenOptions, SessionError};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, AuditSink, Clock, NoAudit, NoStore, RegistryStore, Sheets};
use std::future::Future;
use std::sync::Arc;

/// How much of a relay's traffic an in-memory stream holds before the writer waits.
const RELAY_BUFFER: usize = 64 * 1024;

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

/// The core, hosted in this process, answering for one app. The registry store, the audit sink,
/// the inference broker and the relay host default to none.
#[derive(Debug)]
pub struct InProcess<P, S, U, K, B = NoBroker, R = NoStore, A = NoAudit, H = NoRelays> {
    service: Arc<AccountService<P, S, U, K, R, A>>,
    app: AppId,
    broker: B,
    relays: H,
}

impl<P, S, U, K, R, A> InProcess<P, S, U, K, NoBroker, R, A, NoRelays> {
    /// `app`'s link to a service this process hosts, with no broker and no relays.
    pub fn new(service: Arc<AccountService<P, S, U, K, R, A>>, app: AppId) -> Self {
        Self {
            service,
            app,
            broker: NoBroker,
            relays: NoRelays,
        }
    }
}

impl<P, S, U, K, B, R, A, H> InProcess<P, S, U, K, B, R, A, H> {
    /// The same link with `broker` serving its inference sessions.
    pub fn with_broker<N: SessionHost>(self, broker: N) -> InProcess<P, S, U, K, N, R, A, H> {
        InProcess {
            service: self.service,
            app: self.app,
            broker,
            relays: self.relays,
        }
    }

    /// The same link with `relays` running the relays of its authenticated streams.
    pub fn with_relays<N: RelayHost>(self, relays: N) -> InProcess<P, S, U, K, B, R, A, N> {
        InProcess {
            service: self.service,
            app: self.app,
            broker: self.broker,
            relays,
        }
    }
}

impl<P, S, U, K, B, R, A, H> Transport for InProcess<P, S, U, K, B, R, A, H>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    B: SessionHost,
    R: RegistryStore,
    A: AuditSink,
    H: RelayHost,
{
    type Session = B::Session;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        Ok(self.service.handle(&self.app, request).await)
    }

    async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        match self
            .service
            .open_authenticated(&self.app, grant, endpoint)
            .await
        {
            Ok(plan) => {
                let (app_end, relay_end) = duplex(RELAY_BUFFER);
                self.relays.run(plan, relay_end);
                Ok(Relayed::Stream(AuthenticatedStream::Memory(app_end)))
            }
            Err(refusal) => Ok(Relayed::Refused(refusal)),
        }
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
