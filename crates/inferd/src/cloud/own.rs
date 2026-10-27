//! Servers of the person's own: accounts of the provider `openai-compatible`, each a server at an
//! address the person typed (a gateway, a llama.cpp or vLLM on their network) that speaks the
//! OpenAI chat protocol. Each such account is its own provider here: its models are the ones the
//! server lists (`GET <address>/models`), at add time and on `Rescan`, and its address is where a
//! turn goes, instead of the fixed table of [`Doors`](super::wire::Doors).
//!
//! * [`OwnServer`] is what the accounts give (the account, its name, where it is);
//! * [`Place`] is where and how: the address's host, port and base path, and whether the
//!   connection is TLS, plain on this computer, or plain elsewhere (a computer of the person's
//!   own on their tailnet, which stoker sends no key to);
//! * [`OwnBook`] keeps what each server listed, in memory, as hosted models with no limit known:
//!   the context window and the longest reply are 0, meaning not known, and the price is 0.
//!
//! A list that did not come is kept as nothing for [`FAILED_RETRY`] and then asked again, so a
//! server that was down when it was added is found when it is up, and one that hangs is not
//! asked at every session.

use super::accountd::AccountVerdict;
use super::models::{RemoteModel, card_of};
use super::picker::{Availability, Choice};
use super::wire::Door;
use crate::probe::names::id_of;
use model_catalog::{Locality, ModelEntry, Reach};
use model_http::{
    AuthHeader, BodySink, ChunkFlow, Exchange, Framing, HostName, HttpClient, HttpEndpoint,
    HttpStatus, HttpTarget, Port, Proxy, ResponseHead, RouteRoot, Secret, Timeouts, TlsRoots,
    Transport, UrlPath, Verb, WaitMs,
};
use porter_core::{AccountId, ModelId, SecretText, ServiceEndpoint, UnixSeconds, UrlScheme};
use porter_infer::Takes;
use porter_router::catalog::{capabilities_of, parse_entry_text};
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

/// The provider file whose accounts are servers of the person's own.
pub const OWN_PROVIDER: &str = "openai-compatible";

/// The group the models of such servers are listed under in the model picker.
pub const OWN_COMPANY: &str = "Other providers";

/// How long a list that did not come is taken for the answer before it is asked for again.
pub const FAILED_RETRY: i64 = 60;

/// The most models kept from one server.
const MOST_MODELS: usize = 200;

/// The most of the list read, in bytes.
const MOST_BODY: usize = 4 * 1024 * 1024;

/// How long the list waits at each stage: a session opening or a picker drawing waits no longer
/// for a server that hangs.
const LIST_WAIT: Timeouts = Timeouts {
    connect: WaitMs(10_000),
    first_byte: WaitMs(15_000),
    idle: WaitMs(15_000),
};

/// How the connection to a server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// TLS from the first byte (`https`).
    Tls,
    /// Plain `http` to this computer.
    PlainLoopback,
    /// Plain `http` to a computer of the person's own: stoker sends no key over it.
    Plain,
}

/// Where a server is and how it is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    door: Door,
    link: Link,
}

impl Place {
    /// A provider at a fixed `door`, over TLS.
    pub fn tls(door: Door) -> Self {
        Self {
            door,
            link: Link::Tls,
        }
    }

    /// The place the account's endpoint names; none for a scheme that is not `http` or `https`.
    pub fn of(endpoint: &ServiceEndpoint) -> Option<Self> {
        let origin = endpoint.url.origin();
        let link = match origin.scheme {
            UrlScheme::Https => Link::Tls,
            UrlScheme::Http if origin.is_loopback() => Link::PlainLoopback,
            UrlScheme::Http => Link::Plain,
            _ => return None,
        };
        Some(Self {
            door: Door {
                host: origin.host.clone(),
                port: origin.port,
                base: endpoint.url.path().trim_end_matches('/').to_owned(),
            },
            link,
        })
    }

    /// How it is reached.
    pub fn link(&self) -> Link {
        self.link
    }

    /// Where it is.
    pub fn door(&self) -> &Door {
        &self.door
    }

    /// The endpoint of one request here, presenting `key` as a bearer token; an empty key is no
    /// header at all (a server of the person's own may need none).
    pub fn endpoint(&self, key: &SecretText, timeouts: Timeouts) -> HttpEndpoint {
        let host = HostName(self.door.host.clone());
        let port = Port(self.door.port);
        let target = match self.link {
            Link::Tls => HttpTarget::Tls { host, port },
            Link::PlainLoopback | Link::Plain => HttpTarget::Tcp { host, port },
        };
        let secret = Secret(key.expose().to_owned());
        let auth = match (key.expose().is_empty(), self.link) {
            (true, _) => AuthHeader::None,
            (false, Link::PlainLoopback) => AuthHeader::LoopbackBearer(secret),
            (false, Link::Tls | Link::Plain) => AuthHeader::Bearer(secret),
        };
        HttpEndpoint {
            target,
            proxy: Proxy::Direct,
            base: UrlPath(self.door.base.clone()),
            auth,
            headers: Vec::new(),
            timeouts,
        }
    }
}

/// An account that is a server of the person's own, as the accounts give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnServer {
    /// The account.
    pub account: AccountId,
    /// What the person calls it.
    pub label: String,
    /// Where it is.
    pub place: Place,
}

impl OwnServer {
    /// The server the account `account` keeps at `endpoints`; none when it keeps no address.
    pub fn of(account: AccountId, label: String, endpoints: &[ServiceEndpoint]) -> Option<Self> {
        let endpoint = endpoints
            .iter()
            .find(|endpoint| endpoint.family == porter_core::Family::ChatCompletions)?;
        Some(Self {
            account,
            label,
            place: Place::of(endpoint)?,
        })
    }
}

/// One model a server listed, as a hosted entry with nothing known of its limits.
#[derive(Debug, Clone)]
pub struct Listed {
    /// The name the server serves it under, which a request names.
    pub name: String,
    id: ModelId,
    entry: ModelEntry,
    reach: Reach,
}

impl Listed {
    /// The model `name` of the server that is account `account`: its entry, which routing and
    /// the turn code read, and its one reach. None when the name makes no id or the entry does
    /// not parse.
    pub fn new(account: &AccountId, name: &str) -> Option<Self> {
        let id = id_of(name)?;
        let quoted = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
        let text = format!(
            r#"id = {id}
label = {label}
licence = {{ kind = "proprietary" }}
family = "own"
cold_start_estimate_s = 0
source = {{ kind = "hosted" }}
vram = {{ weights_mib = 0, kv_per_1k_ctx_mib = 0, overhead_mib = 0 }}
locality = {{ kind = "remote", v = {{ reach = [ {{ provider = {provider}, model = {name}, price = {{ input_per_mtok = 0, output_per_mtok = 0 }}, wire = "open_ai_compat" }} ] }} }}
inputs = ["text"]
outputs = ["text"]
text_out = {{ tools = "native", structured = [], reasoning = "absent", streaming = "present", context = 0, max_output = 0 }}
"#,
            id = quoted(id.as_str()),
            label = quoted(name),
            provider = quoted(account.as_str()),
            name = quoted(name),
        );
        let entry = parse_entry_text(&text).ok()?;
        let reach = match &entry.locality {
            Locality::Remote { reach } => reach.first()?.clone(),
            Locality::OnDevice => return None,
        };
        Some(Self {
            name: name.to_owned(),
            id,
            entry,
            reach,
        })
    }

    /// The model's id here.
    pub fn id(&self) -> &ModelId {
        &self.id
    }
}

/// What a server listed.
#[derive(Debug, Clone)]
enum Outcome {
    /// Its models, in its order.
    Listed(Vec<Listed>),
    /// It did not answer a readable list.
    Failed,
}

#[derive(Debug, Clone)]
struct Listing {
    place: Place,
    outcome: Outcome,
    at: UnixSeconds,
}

/// What each server of the person's own listed, in memory.
#[derive(Debug, Clone, Default)]
pub struct OwnBook(Arc<RwLock<BTreeMap<AccountId, Listing>>>);

impl OwnBook {
    fn read(&self) -> BTreeMap<AccountId, Listing> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Where the server of `account` is.
    pub fn place_of(&self, account: &AccountId) -> Option<Place> {
        self.read()
            .get(account)
            .map(|listing| listing.place.clone())
    }

    /// Whether `account` lists a model called `model`.
    pub fn holds(&self, account: &AccountId, model: &ModelId) -> bool {
        match self.read().get(account).map(|listing| &listing.outcome) {
            Some(Outcome::Listed(models)) => models.iter().any(|one| one.id() == model),
            Some(Outcome::Failed) | None => false,
        }
    }

    /// Whether what is kept of `server` still stands at `now`: it was listed from the same place
    /// (a list is asked for again when the address changes), or it failed less than
    /// [`FAILED_RETRY`] seconds ago.
    pub fn is_current(&self, server: &OwnServer, now: UnixSeconds) -> bool {
        match self.read().get(&server.account) {
            Some(listing) if listing.place == server.place => match listing.outcome {
                Outcome::Listed(_) => true,
                Outcome::Failed => now.0 - listing.at.0 < FAILED_RETRY,
            },
            _ => false,
        }
    }

    /// Keeps what `server` listed (`None`: it did not answer a list) as of `now`.
    pub fn record(&self, server: &OwnServer, names: Option<Vec<String>>, now: UnixSeconds) {
        let outcome = match names {
            Some(names) => Outcome::Listed(
                names
                    .iter()
                    .filter_map(|name| Listed::new(&server.account, name))
                    .collect(),
            ),
            None => Outcome::Failed,
        };
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                server.account.clone(),
                Listing {
                    place: server.place.clone(),
                    outcome,
                    at: now,
                },
            );
    }

    /// Forgets the accounts that are not among `servers` (removed, or no longer a server).
    pub fn keep(&self, servers: &[OwnServer]) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|account, _| servers.iter().any(|server| server.account == *account));
    }

    /// The listed models of the accounts in `verdicts`, as hosted models: each account's models
    /// carry the verdict the app holds on that account.
    pub fn models(&self, verdicts: &[AccountVerdict]) -> Vec<RemoteModel> {
        let book = self.read();
        verdicts
            .iter()
            .filter_map(|verdict| Some((verdict, book.get(&verdict.account)?)))
            .flat_map(|(verdict, listing)| {
                let Outcome::Listed(models) = &listing.outcome else {
                    return Vec::new();
                };
                models
                    .iter()
                    .filter_map(|listed| {
                        Some(RemoteModel {
                            card: card_of(&listed.entry, &verdict.account, &listed.reach)?,
                            entry: listed.entry.clone(),
                            reach: listed.reach.clone(),
                            verdict: verdict.verdict.clone(),
                            state: verdict.state,
                        })
                    })
                    .collect()
            })
            .collect()
    }

    /// The listed models of the accounts in `verdicts`, as the model picker lists them:
    /// "<model> on <account>", under one group.
    pub fn choices(&self, verdicts: &[AccountVerdict]) -> Vec<Choice> {
        let book = self.read();
        verdicts
            .iter()
            .filter_map(|verdict| Some((verdict, book.get(&verdict.account)?)))
            .flat_map(|(verdict, listing)| {
                let Outcome::Listed(models) = &listing.outcome else {
                    return Vec::new();
                };
                let account = verdict
                    .label
                    .clone()
                    .unwrap_or_else(|| verdict.account.to_string());
                models
                    .iter()
                    .map(|listed| Choice {
                        value: format!("{}/{}", verdict.account, listed.id()),
                        label: format!("{} on {account}", listed.name),
                        company: OWN_COMPANY.to_owned(),
                        available: Availability::Reachable,
                        hint: None,
                        takes: Takes::of(&capabilities_of(&listed.entry)),
                    })
                    .collect()
            })
            .collect()
    }
}

/// The status of a response and its body, to a bound.
#[derive(Debug, Default)]
struct Reply {
    body: Vec<u8>,
}

impl BodySink for Reply {
    fn head(&mut self, _head: &ResponseHead) -> ChunkFlow {
        ChunkFlow::Continue
    }

    fn chunk(&mut self, bytes: &[u8]) -> ChunkFlow {
        let room = MOST_BODY.saturating_sub(self.body.len());
        self.body.extend_from_slice(&bytes[..bytes.len().min(room)]);
        match self.body.len() < MOST_BODY {
            true => ChunkFlow::Continue,
            false => ChunkFlow::Stop,
        }
    }
}

/// The model names a `GET /models` body lists (`{"data": [{"id": "..."}]}`), in its order, each
/// once; none when the body is not such a list.
pub fn names_in(body: &[u8]) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let listed = value.get("data")?.as_array()?;
    let mut names: Vec<String> = Vec::new();
    for name in listed
        .iter()
        .filter_map(|model| model.get("id")?.as_str())
        .filter(|name| id_of(name).is_some())
    {
        if names.len() == MOST_MODELS {
            break;
        }
        if !names.iter().any(|seen| seen == name) {
            names.push(name.to_owned());
        }
    }
    Some(names)
}

/// Asks the server at `place` for its models with `key` (`GET <base>/models`); none when it does
/// not answer a readable list. The key goes to this server only, in the one request.
pub async fn list(place: &Place, key: &SecretText, roots: &TlsRoots) -> Option<Vec<String>> {
    let exchange = Exchange {
        verb: Verb::Get,
        root: RouteRoot::Base,
        path: UrlPath("/models".to_owned()),
        body: None,
        framing: Framing::Whole,
    };
    let mut reply = Reply::default();
    let sent = HttpClient::with_roots(place.endpoint(key, LIST_WAIT), roots.clone())
        .exchange(&exchange, &mut reply)
        .await;
    match sent {
        Ok(HttpStatus(200..=299)) => names_in(&reply.body),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
