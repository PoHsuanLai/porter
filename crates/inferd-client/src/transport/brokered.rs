//! The carrier for an app that hosts its inference itself: the sessions come from a
//! [`SessionHost`] the app gives (`engines::EngineHost`, or its own), opened for the app the host
//! names.

use super::Transport;
use super::broker::SessionHost;
use crate::error::TransportError;
use porter_capability::Tier;
use porter_core::{AppId, DataClass};
use porter_infer::{OpenOptions, Readiness};
use porter_match::Need;

/// A [`SessionHost`], answering for one app.
#[derive(Debug)]
pub struct Brokered<B> {
    app: AppId,
    broker: B,
}

impl<B> Brokered<B> {
    /// `app`'s link to a broker this process hosts.
    pub fn new(app: AppId, broker: B) -> Self {
        Self { app, broker }
    }
}

impl<B: SessionHost> Transport for Brokered<B> {
    type Session = B::Session;

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
