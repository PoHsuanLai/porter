//! One engine the app points at: where it is, what it serves, how it is spoken to and whether a
//! key goes with every request.

use model_openai_compat::Flavor;
use porter_core::{AccountId, ModelId, Tokens};
use std::fmt;

/// What the app calls an engine in its routing table: the id grammar (`[a-z0-9]` then
/// `[a-z0-9._-]`, at most 64 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EngineId(String);

impl EngineId {
    /// The id named `text`.
    pub fn parse(text: &str) -> Result<Self, EngineError> {
        match porter_core::is_id(text) {
            true => Ok(Self(text.to_owned())),
            false => Err(EngineError::Id(text.to_owned())),
        }
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why an engine, or the table over engines, is not usable. Said when the host is built, never
/// when a session is opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// Not an engine id.
    #[error("{0:?} is not an engine id")]
    Id(String),
    /// Not an `http://` or `https://` address with a host.
    #[error("{0:?} is not an http or https engine address")]
    Url(String),
    /// The served model name is not one porter can name (no letters or digits).
    #[error("{0:?} names no model")]
    Model(String),
    /// Two engines have the same id.
    #[error("engine {0} is given twice")]
    Duplicate(EngineId),
    /// A route names an engine the host was not given.
    #[error("a route names engine {0}, which was not given")]
    Unknown(EngineId),
    /// A key would travel in clear text to a machine that is not this one.
    #[error("engine {0} would send its key over plain http beyond this computer")]
    KeyInTheClear(EngineId),
}

/// How to speak to the server: the dialects of the shared chat-completions wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// llama.cpp's server, and the runtimes inferd probes the same way (Ollama, LM Studio).
    LlamaServer,
    /// OpenAI's own and the companies that follow it (a usage chunk asked for,
    /// `reasoning_effort`).
    OpenAi,
    /// OpenRouter's.
    OpenRouter,
}

impl Dialect {
    pub(crate) fn flavor(self) -> Flavor {
        match self {
            Dialect::LlamaServer => Flavor::LlamaServer,
            Dialect::OpenAi => Flavor::LiteLlm,
            Dialect::OpenRouter => Flavor::OpenRouter,
        }
    }
}

/// Whether the engine wants a key with every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyUse {
    /// No key: a runtime on this computer or the user's network.
    None,
    /// `Authorization: Bearer <key>`, the key for the engine's account from the app's
    /// [`KeySource`](crate::engines::KeySource), fetched for each turn.
    Bearer,
}

/// How an engine is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scheme {
    /// Plain http.
    Http,
    /// TLS, verified against the platform's roots.
    Https,
}

impl Scheme {
    fn port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }
}

/// Where an engine is: `http://host:port/base` or `https://host:port/base`. The base is what
/// every request path goes under (`/v1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineUrl {
    pub(crate) scheme: Scheme,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) base: String,
}

impl EngineUrl {
    /// The address `text`; the port is the scheme's own (80, 443) when it names none.
    pub fn parse(text: &str) -> Result<Self, EngineError> {
        let bad = || EngineError::Url(text.to_owned());
        let (scheme, rest) = match (text.strip_prefix("http://"), text.strip_prefix("https://")) {
            (Some(rest), _) => (Scheme::Http, rest),
            (_, Some(rest)) => (Scheme::Https, rest),
            _ => return Err(bad()),
        };
        let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| bad())?),
            None => (authority, scheme.port()),
        };
        let plain_host = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
        let base = path.trim_end_matches('/');
        match plain_host && !base.contains(['?', '#', ' ']) {
            true => Ok(Self {
                scheme,
                host: host.to_owned(),
                port,
                base: if base.is_empty() {
                    String::new()
                } else {
                    format!("/{base}")
                },
            }),
            false => Err(bad()),
        }
    }

    /// Whether this is this computer: the loopback name or address, which is the one place a key
    /// may go over plain http.
    pub fn is_loopback(&self) -> bool {
        self.host == "localhost" || self.host == "127.0.0.1"
    }
}

/// One engine: an OpenAI-compatible server the app configures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engine {
    /// The id the routing table names it by.
    pub id: EngineId,
    /// The account the answers are served by (`ServedBy::account`, the audit's account), and
    /// whose key the [`KeySource`](crate::engines::KeySource) is asked for.
    pub account: AccountId,
    /// The model as porter names it (`ServedBy::model`).
    pub model: ModelId,
    /// The name in the request's `model` field, as the server knows it (`llama3.2:3b`).
    pub served_name: String,
    /// Where the model runs. Decides which data classes may go to it (the policy's floors).
    pub locality: porter_core::Locality,
    /// Where the server is.
    pub url: EngineUrl,
    /// How it is spoken to.
    pub dialect: Dialect,
    /// The reply limit sent when the app's request names none (`max_tokens` is always sent).
    pub max_output: Tokens,
    /// Whether a key goes with every request.
    pub key: KeyUse,
}

impl Engine {
    /// An engine serving `served_name` at `url`, answered for by `account`, running at
    /// `locality`, with no key. `model` is porter's name for `served_name`
    /// (`Hcompany/Holo-3.1-4B` is `hcompany-holo-3.1-4b`).
    pub fn new(
        id: EngineId,
        account: AccountId,
        served_name: &str,
        url: EngineUrl,
        locality: porter_core::Locality,
        dialect: Dialect,
        max_output: Tokens,
    ) -> Result<Self, EngineError> {
        let model = porter_bridge::model_id_of(served_name)
            .map_err(|_| EngineError::Model(served_name.to_owned()))?;
        Ok(Self {
            id,
            account,
            model,
            served_name: served_name.to_owned(),
            locality,
            url,
            dialect,
            max_output,
            key: KeyUse::None,
        })
    }

    /// The same engine, sending the account's key as a bearer token.
    pub fn with_key(self) -> Self {
        Self {
            key: KeyUse::Bearer,
            ..self
        }
    }
}
