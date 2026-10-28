//! The in-process carrier: the app hosts porter's core itself (mailo standalone, tests). The
//! caller is the app the host names. Feature `in-process`: it is the one carrier that reaches
//! porter-service, porter-secrets and porter-provider.
//!
//! The app keeps its own registry and its own credentials: the service it builds is given a
//! `RegistryStore` (`porter_store::FileStore` over a directory of its own, or nothing) and a
//! `Secrets` store filed under a namespace of its own (`porter_secrets::Namespace`, such as
//! `mailo/<profile>`). This crate builds neither; the host names them where it builds the
//! service.
//!
//! Accounts are hosted: the service answers every call. Inference is `inferd-client`'s
//! (`Brokered`).

use super::Transport;
use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::error::TransportError;
use crate::relays::{NoRelays, RelayHost};
use porter_core::DataClass;
use porter_core::GrantId;
use porter_core::consent::{Availability, Usage};
use porter_core::store::RegistryStore;
use porter_core::stream::duplex;
use porter_core::{AppId, Candidate, EndpointUrl, RelayPlan};
use porter_ids::clock::Clock;
use porter_match::Need;
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::AccountService;
use porter_service_core::audit::AuditSink;
use porter_sheet::host::Sheets;
use porter_wire::Refusal;
use porter_wire::{AccountsReply, AccountsRequest};
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

    /// The accounts that fit `need` for `app` and that it holds a grant for.
    fn candidates(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Vec<Candidate>;

    /// Whether `need` can be met for `app`, revealing no identity.
    fn availability(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Availability;

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

    fn candidates(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Vec<Candidate> {
        AccountService::candidates(self, app, need, class, usage)
    }

    fn availability(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Availability {
        AccountService::availability(self, app, need, class, usage)
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
/// `AccountService<..>`, a [`HostedCore`]); the relay host defaults to none.
#[derive(Debug)]
pub struct InProcess<C, H = NoRelays> {
    service: Arc<C>,
    app: AppId,
    relays: H,
}

impl<C> InProcess<C, NoRelays> {
    /// `app`'s link to a service this process hosts, with no relays.
    pub fn new(service: Arc<C>, app: AppId) -> Self {
        Self {
            service,
            app,
            relays: NoRelays,
        }
    }
}

impl<C, H> InProcess<C, H> {
    /// The same link with `relays` running its authenticated relays (`PorterRelays`, feature
    /// `relays`, or a host the app writes). Without one, opening an authenticated stream says
    /// nobody is reachable.
    pub fn with_relays<N: RelayHost>(self, relays: N) -> InProcess<C, N> {
        InProcess {
            service: self.service,
            app: self.app,
            relays,
        }
    }
}

impl<C, H> InProcess<C, H>
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

impl<C, H> InProcess<C, H>
where
    C: HostedCore,
{
    /// The accounts that fit `need` for this app and that it holds a grant for.
    pub(crate) fn granted(&self, need: &Need, class: DataClass, usage: Usage) -> Vec<Candidate> {
        self.service.candidates(&self.app, need, class, usage)
    }

    /// Whether `need` can be met for this app, revealing no identity.
    pub(crate) fn availability(&self, need: &Need, class: DataClass, usage: Usage) -> Availability {
        self.service.availability(&self.app, need, class, usage)
    }
}

impl<C, H> Transport for InProcess<C, H>
where
    C: HostedCore,
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
