//! The in-process carrier: the app hosts porter's core itself (mailo standalone, tests). The
//! caller is the app the host names.

use super::Transport;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, AppId, DataClass, Need, Tier};
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, Clock, Prompter};
use std::sync::Arc;

/// The core, hosted in this process, answering for one app.
#[derive(Debug)]
pub struct InProcess<P, S, U, K> {
    service: Arc<AccountService<P, S, U, K>>,
    app: AppId,
}

impl<P, S, U, K> InProcess<P, S, U, K> {
    /// `app`'s link to a service this process hosts.
    pub fn new(service: Arc<AccountService<P, S, U, K>>, app: AppId) -> Self {
        Self { service, app }
    }
}

/// A session with the broker this process hosts.
#[derive(Debug)]
pub struct InProcessSession {
    _private: (),
}

impl InferSession for InProcessSession {
    async fn send(&mut self, _frame: ClientFrame) -> Result<(), SessionError> {
        todo!("hand the frame to the hosted porter_infer::Broker")
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        todo!("the next event the hosted broker pushed")
    }
}

impl<P: Provider, S: Secrets, U: Prompter, K: Clock> Transport for InProcess<P, S, U, K> {
    type Session = InProcessSession;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        Ok(self.service.handle(&self.app, request).await)
    }

    async fn open(
        &self,
        _need: &Need,
        _class: DataClass,
        _tier: Tier,
    ) -> Result<InProcessSession, TransportError> {
        todo!("host porter_infer::Broker beside the service")
    }
}
