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
use porter_core::{AccountsReply, AccountsRequest, AppId, EndpointUrl, GrantId};
#[cfg(feature = "infer")]
use porter_core::{DataClass, Need, Tier};
#[cfg(feature = "infer")]
use porter_infer::{OpenOptions, Readiness};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, AuditSink, Clock, NoAudit, NoStore, RegistryStore, Sheets};
use std::sync::Arc;

/// How much of a relay's traffic an in-memory stream holds before the writer waits.
const RELAY_BUFFER: usize = 64 * 1024;

/// The core, hosted in this process, answering for one app. The registry store, the audit sink,
/// the inference broker and the relay host default to none.
#[derive(Debug)]
pub struct InProcess<P, S, U, K, B = NoBroker, R = NoStore, A = NoAudit, H = NoRelays> {
    service: Arc<AccountService<P, S, U, K, R, A>>,
    app: AppId,
    #[cfg_attr(not(feature = "infer"), allow(dead_code))]
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
    /// The same link with `broker` serving its inference sessions (feature `infer`).
    #[cfg(feature = "infer")]
    pub fn with_broker<N: SessionHost>(self, broker: N) -> InProcess<P, S, U, K, N, R, A, H> {
        InProcess {
            service: self.service,
            app: self.app,
            broker,
            relays: self.relays,
        }
    }
}

/// The accounts half, the same with and without inference.
impl<P, S, U, K, B, R, A, H> InProcess<P, S, U, K, B, R, A, H>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    R: RegistryStore,
    A: AuditSink,
    H: RelayHost,
{
    async fn call_service(&self, request: AccountsRequest) -> AccountsReply {
        self.service.handle(&self.app, request).await
    }

    async fn relayed_authenticated(&self, grant: &GrantId, endpoint: &EndpointUrl) -> Relayed {
        match self
            .service
            .open_authenticated(&self.app, grant, endpoint)
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
        match self.service.open_linked(&self.app, grant, origin).await {
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
impl<P, S, U, K, B, R, A, H> Transport for InProcess<P, S, U, K, B, R, A, H>
where
    P: Provider,
    S: Secrets,
    U: Sheets,
    K: Clock,
    B: Send + Sync,
    R: RegistryStore,
    A: AuditSink,
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
