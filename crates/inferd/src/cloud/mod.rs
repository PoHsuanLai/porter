//! Hosted models: the curated remote entries of stoker's catalogue, reached through accounts the
//! person gave an app a grant on.
//!
//! * [`accountd`]: the seam to accountd (`Peer.Verdicts` for which accounts an app holds a grant
//!   on, `Peer.ResolveKey` for a key on a sealed descriptor).
//! * [`models`]: the entries an app's grants reach, each through one reach, as routing's cards.
//! * [`picker`]: the hosted models as the model picker lists them.
//! * [`wire`]: where each provider is and how a request is shaped for it.
//! * [`transport`]: stoker's TLS `HttpClient` behind inferd's body shaping.
//! * [`spend`]: the meter and the caps.
//! * [`turn`]: one chat turn to a hosted model: key, request, reply, meter.
//!
//! [`Cloud`] holds what one daemon shares: the entries, the seam to accountd, the ledger, the
//! clock, the providers' addresses and the trusted roots. An app that is not named (a session
//! opened through `Engines::router`) is served nothing from here.

pub mod accountd;
pub mod models;
pub mod picker;
pub mod spend;
pub mod transport;
pub mod turn;
pub mod wire;

use crate::clock::Clock;
use crate::settings::SpendLine;
use accountd::{AccountVerdict, Accountd, AccountdFault};
use model_catalog::{ModelEntry, ProviderId, Wire};
use model_http::TlsRoots;
use model_http::{
    AuthHeader, ExtraHeader, HeaderName, HostName, HttpClient, HttpEndpoint, HttpTarget, Port,
    Proxy, Secret, Timeouts,
};
use model_http::{UrlPath, WaitMs};
use models::{RemoteModel, remote_entries, remote_models};
use porter_core::consent::Usage;
use porter_core::{AppId, DataClass, GrantId, SecretText, UnixSeconds};
use porter_infer::SpendVerdict;
use spend::{Ledger, estimate};
use std::sync::Arc;
use transport::ShapedTransport;
use wire::{BodyShape, Doors};

/// Waits for a hosted model: the connection, the first byte (a long prompt, a model that thinks
/// first), and between chunks.
const TIMEOUTS: Timeouts = Timeouts {
    connect: WaitMs(10_000),
    first_byte: WaitMs(180_000),
    idle: WaitMs(90_000),
};

struct Inner {
    accountd: Arc<dyn Accountd>,
    entries: Vec<ModelEntry>,
    ledger: Ledger,
    clock: Arc<dyn Clock>,
    doors: Doors,
    roots: TlsRoots,
}

/// The hosted models of one daemon.
#[derive(Clone)]
pub struct Cloud(Arc<Inner>);

impl std::fmt::Debug for Cloud {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cloud")
            .field("entries", &self.0.entries.len())
            .finish_non_exhaustive()
    }
}

impl Cloud {
    /// The hosted models among `catalog`, reached through `accountd`, metered in `ledger`, at the
    /// providers' real addresses with the platform's roots.
    pub fn new(
        accountd: Arc<dyn Accountd>,
        catalog: &[ModelEntry],
        ledger: Ledger,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self(Arc::new(Inner {
            accountd,
            entries: remote_entries(catalog),
            ledger,
            clock,
            doors: Doors::real(),
            roots: TlsRoots::Platform,
        }))
    }

    /// The same, with the providers somewhere else (a test's loopback fake) and `roots` trusted.
    /// Called right after `new`, before the value is shared.
    pub fn at(mut self, doors: Doors, roots: TlsRoots) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.0) {
            inner.doors = doors;
            inner.roots = roots;
        }
        self
    }

    /// The hosted entries, in catalogue order.
    pub fn entries(&self) -> &[ModelEntry] {
        &self.0.entries
    }

    /// Now.
    pub fn now(&self) -> UnixSeconds {
        self.0.clock.now()
    }

    /// The meter.
    pub fn ledger(&self) -> &Ledger {
        &self.0.ledger
    }

    /// The accounts `app` has a verdict on for data of `class`; none when accountd cannot be
    /// reached (nothing hosted is served then).
    pub async fn accounts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
    ) -> Vec<AccountVerdict> {
        self.0
            .accountd
            .verdicts(app, class, usage)
            .await
            .unwrap_or_default()
    }

    /// The hosted models `app` is offered for data of `class`.
    pub async fn models(&self, app: &AppId, class: DataClass, usage: Usage) -> Vec<RemoteModel> {
        remote_models(&self.0.entries, &self.accounts(app, class, usage).await)
    }

    /// What the caps say about one more turn of `app` on `model`.
    pub fn spend(&self, line: SpendLine, app: &AppId, model: &RemoteModel) -> SpendVerdict {
        self.0.ledger.verdict(
            line,
            app,
            &model.card.account,
            estimate(&model.price()),
            self.now(),
        )
    }

    /// The key behind `grant`, fetched now.
    pub async fn key(&self, grant: &GrantId) -> Result<SecretText, AccountdFault> {
        self.0.accountd.key(grant).await
    }

    /// The transport of one turn on `model`'s reach, authenticated with `key`; `None` when this
    /// build has no address for the provider.
    pub fn transport(
        &self,
        model: &RemoteModel,
        key: &SecretText,
        shape: BodyShape,
    ) -> Option<ShapedTransport> {
        let door = self.0.doors.of(&model.reach.provider)?;
        let endpoint = HttpEndpoint {
            target: HttpTarget::Tls {
                host: HostName(door.host.clone()),
                port: Port(door.port),
            },
            proxy: Proxy::Direct,
            base: UrlPath(door.base.clone()),
            auth: AuthHeader::Bearer(Secret(key.expose().to_owned())),
            headers: Vec::new(),
            timeouts: TIMEOUTS,
        };
        Some(ShapedTransport::new(endpoint, self.0.roots.clone(), shape))
    }

    /// A client for an agent's request to `provider`, authenticated with `key` the way the wire
    /// asks (`x-api-key` for Anthropic's Messages, a bearer for chat completions) and sending
    /// `extra` headers beside it; `None` when this build has no address for the provider. Built
    /// for one request and dropped with it, like a turn's transport.
    pub fn agent_client(
        &self,
        provider: &ProviderId,
        wire: Wire,
        key: &SecretText,
        extra: Vec<(String, String)>,
    ) -> Option<HttpClient> {
        let door = self.0.doors.of(provider)?;
        let secret = Secret(key.expose().to_owned());
        let endpoint = HttpEndpoint {
            target: HttpTarget::Tls {
                host: HostName(door.host.clone()),
                port: Port(door.port),
            },
            proxy: Proxy::Direct,
            base: UrlPath(door.base.clone()),
            auth: match wire {
                Wire::AnthropicMessages => AuthHeader::Header {
                    name: HeaderName("x-api-key".to_owned()),
                    value: secret,
                },
                Wire::OpenAiCompat => AuthHeader::Bearer(secret),
            },
            headers: extra
                .into_iter()
                .map(|(name, value)| ExtraHeader {
                    name: HeaderName(name),
                    value: Secret(value),
                })
                .collect(),
            timeouts: TIMEOUTS,
        };
        Some(HttpClient::with_roots(endpoint, self.0.roots.clone()))
    }
}
