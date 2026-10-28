//! Hosted models: the curated remote entries of stoker's catalogue, reached through accounts the
//! person gave an app a grant on.
//!
//! * [`accountd`]: the seam to accountd (`Peer.Verdicts` for which accounts an app holds a grant
//!   on, `Peer.ResolveKey` for a key on a sealed descriptor).
//! * [`models`]: the entries an app's grants reach, each through one reach, as routing's cards.
//! * [`own`]: the servers of the person's own (accounts of `openai-compatible`): where each is,
//!   and the models each lists, as hosted models with no limit known.
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
pub mod own;
pub mod picker;
pub mod spend;
pub mod transport;
pub mod turn;
pub mod wire;

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
use own::{OwnBook, OwnServer, Place};
use porter_core::consent::Usage;
use porter_core::{AccountId, GrantId};
use porter_core::{AppId, DataClass, SecretText};
use porter_ids::UnixSeconds;
use porter_ids::clock::Clock;
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

/// Which servers of the person's own a look asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    /// The ones whose list is not known.
    IfNeeded,
    /// All of them.
    Again,
}

struct Inner {
    accountd: Arc<dyn Accountd>,
    entries: Vec<ModelEntry>,
    ledger: Ledger,
    clock: Arc<dyn Clock>,
    doors: Doors,
    roots: TlsRoots,
    /// What the servers of the person's own listed: each is its own provider, at its own address.
    own: OwnBook,
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
            own: OwnBook::default(),
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

    /// The hosted models `app` is offered for data of `class`: the catalogue's, and the ones the
    /// servers of the person's own listed (asked for the first time an account is seen).
    pub async fn models(&self, app: &AppId, class: DataClass, usage: Usage) -> Vec<RemoteModel> {
        let accounts = self.accounts(app, class, usage).await;
        self.look_at_servers(Look::IfNeeded).await;
        let mut found = remote_models(&self.0.entries, &accounts);
        found.extend(self.0.own.models(&accounts));
        found
    }

    /// The models of the servers of the person's own, as the model picker lists them, for the
    /// accounts in `accounts`.
    pub async fn own_choices(&self, accounts: &[AccountVerdict]) -> Vec<picker::Choice> {
        self.look_at_servers(Look::IfNeeded).await;
        self.0.own.choices(accounts)
    }

    /// Asks every server of the person's own for its models again (`Rescan`).
    pub async fn relist(&self) {
        self.look_at_servers(Look::Again).await;
    }

    /// Whether `model` is one a server of the person's own lists: a model with no engine to start.
    pub fn lists(&self, model: &porter_infer::ModelRef) -> bool {
        self.0.own.holds(&model.account, &model.model)
    }

    /// Asks the servers of the person's own for their models: all of them, or only the ones whose
    /// list is not known (`IfNeeded`: new, moved, or a failed look long enough ago). A server
    /// whose key cannot be read now is left for the next look.
    async fn look_at_servers(&self, look: Look) {
        let servers = self.0.accountd.servers().await;
        self.0.own.keep(&servers);
        for server in servers {
            let known = look == Look::IfNeeded && self.0.own.is_current(&server, self.now());
            if !known {
                self.list(&server).await;
            }
        }
    }

    async fn list(&self, server: &OwnServer) {
        let Ok(key) = self.0.accountd.server_key(&server.account).await else {
            return;
        };
        let names = own::list(&server.place, &key, &self.0.roots).await;
        drop(key);
        self.0.own.record(server, names, self.now());
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
        let endpoint = self
            .place_of(&model.reach.provider)?
            .endpoint(key, TIMEOUTS);
        Some(ShapedTransport::new(endpoint, self.0.roots.clone(), shape))
    }

    /// Where `provider` is: the fixed table's address, else the address of the server of the
    /// person's own that is the account of that name.
    fn place_of(&self, provider: &ProviderId) -> Option<Place> {
        match self.0.doors.of(provider) {
            Some(door) => Some(Place::tls(door.clone())),
            None => self.0.own.place_of(&AccountId::parse(&provider.0).ok()?),
        }
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
