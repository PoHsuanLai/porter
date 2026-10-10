//! The in-process carrier: the app hosts porter's core itself (mailo standalone, tests). The
//! caller is the app the host names. Feature `in-process`: it is the one carrier that reaches
//! porter-service, porter-secrets and porter-provider.
//!
//! Accounts are hosted: the service answers every call. Inference is hosted only if the app
//! hands in a broker ([`SessionHost`], through [`InProcess::with_broker`]): porter's own `Broker`
//! is not built, and an app that hosts accounts alone has no inferd, which a session says as it
//! does when inferd is not running: `TransportError::Unreachable`, so a caller degrades as it
//! would on the bus.

use super::Transport;
use super::broker::NoBroker;
#[cfg(feature = "infer")]
use super::broker::SessionHost;
use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::error::TransportError;
use crate::relays::{NoRelays, RelayHost};
use porter_core::stream::duplex;
use porter_core::wire::Refusal;
use porter_core::{AccountsReply, AccountsRequest, AppId, EndpointUrl, GrantId, RelayPlan};
#[cfg(feature = "infer")]
use porter_core::{DataClass, Need, Tier};
#[cfg(feature = "infer")]
use porter_infer::{OpenOptions, Readiness};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, AuditSink, Clock, RegistryStore, Sheets};
use std::future::Future;
use std::sync::Arc;

/// How much of a relay's traffic an in-memory stream holds before the writer waits.
const RELAY_BUFFER: usize = 64 * 1024;

/// The core an app hosts: what [`InProcess`] needs of the service it answers from. porter's
/// [`AccountService`] is the one implementation, so `InProcess` names the service as one type
/// rather than the six parts (providers, secrets, sheets, clock, store, audit sink) it is built
/// from. The parts are the service's own to choose; an app names them where it builds the
/// service, and never again here.
pub trait HostedCore: Send + Sync {
    /// One request from `app`, answered.
    fn answer(
        &self,
        app: &AppId,
        request: AccountsRequest,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// The plan for a relay that authenticates to one endpoint of a granted account, or why not.
    fn relay_authenticated(
        &self,
        app: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send;

    /// The plan for a relay to a linked origin of a granted account, or why not.
    fn relay_linked(
        &self,
        app: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send;
}

impl<P, S, U, K, R, A> HostedCore for AccountService<P, S, U, K, R, A>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
{
    async fn answer(&self, app: &AppId, request: AccountsRequest) -> AccountsReply {
        self.handle(app, request).await
    }

    async fn relay_authenticated(
        &self,
        app: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        self.open_authenticated(app, grant, endpoint).await
    }

    async fn relay_linked(
        &self,
        app: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        self.open_linked(app, grant, origin).await
    }
}

/// The core, hosted in this process, answering for one app. `C` is the service (porter's
/// `AccountService<..>`, a [`HostedCore`]); the inference broker and the relay host default to
/// none. This was `InProcess<P, S, U, K, B, R, A, H>`; `InProcess::new(service, app)` is
/// unchanged.
#[derive(Debug)]
pub struct InProcess<C, B = NoBroker, H = NoRelays> {
    service: Arc<C>,
    app: AppId,
    #[cfg_attr(not(feature = "infer"), allow(dead_code))]
    broker: B,
    relays: H,
}

impl<C> InProcess<C, NoBroker, NoRelays> {
    /// `app`'s link to a service this process hosts, with no broker and no relays.
    pub fn new(service: Arc<C>, app: AppId) -> Self {
        Self {
            service,
            app,
            broker: NoBroker,
            relays: NoRelays,
        }
    }
}

impl<C, B, H> InProcess<C, B, H> {
    /// The same link with `broker` serving its inference sessions (feature `infer`).
    #[cfg(feature = "infer")]
    pub fn with_broker<N: SessionHost>(self, broker: N) -> InProcess<C, N, H> {
        InProcess {
            service: self.service,
            app: self.app,
            broker,
            relays: self.relays,
        }
    }
}

/// The accounts half, the same with and without inference.
impl<C, B, H> InProcess<C, B, H>
where
    C: HostedCore,
    H: RelayHost,
{
    async fn call_service(&self, request: AccountsRequest) -> AccountsReply {
        self.service.answer(&self.app, request).await
    }

    async fn relayed_authenticated(&self, grant: &GrantId, endpoint: &EndpointUrl) -> Relayed {
        match self
            .service
            .relay_authenticated(&self.app, grant, endpoint)
            .await
        {
            Ok(plan) => {
                let (app_end, relay_end) = duplex(RELAY_BUFFER);
                self.relays.run(plan, relay_end);
                Relayed::Stream(AuthenticatedStream::Memory(app_end))
            }
            Err(refusal) => Relayed::Refused(refusal),
        }
    }

    async fn relayed_linked(&self, grant: &GrantId, origin: &EndpointUrl) -> Relayed {
        match self.service.relay_linked(&self.app, grant, origin).await {
            Ok(plan) => {
                let (app_end, relay_end) = duplex(RELAY_BUFFER);
                self.relays.run(plan, relay_end);
                Relayed::Stream(AuthenticatedStream::Memory(app_end))
            }
            Err(refusal) => Relayed::Refused(refusal),
        }
    }
}

/// Accounts alone: no broker to name.
#[cfg(not(feature = "infer"))]
impl<C, B, H> Transport for InProcess<C, B, H>
where
    C: HostedCore,
    B: Send + Sync,
    H: RelayHost,
{
    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        Ok(self.call_service(request).await)
    }

    async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        Ok(self.relayed_authenticated(grant, endpoint).await)
    }

    async fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        Ok(self.relayed_linked(grant, origin).await)
    }
}

#[cfg(feature = "infer")]
impl<C, B, H> Transport for InProcess<C, B, H>
where
    C: HostedCore,
    B: SessionHost,
    H: RelayHost,
{
    type Session = B::Session;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        Ok(self.call_service(request).await)
    }

    async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        Ok(self.relayed_authenticated(grant, endpoint).await)
    }

    async fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        Ok(self.relayed_linked(grant, origin).await)
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
    async fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<Readiness, TransportError> {
        self.broker
            .prepare(&self.app, need, class, tier, options)
            .await
    }
}
