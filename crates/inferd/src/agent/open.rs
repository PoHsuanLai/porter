//! Opening an endpoint: the checks (setting, program, protocols, route, grant, policy), then the
//! listener and the token.

use super::refusal::{Cause, Refusal};
use super::route::{Models, Route, Target, admits, agent_app};
use super::session::{Agents, Ctx};
use super::token::{Token, random_hex};
use crate::cloud::models::{known_providers, provider_of};
use crate::engines::Engines;
use crate::settings::AgentEndpoint;
use model_catalog::{ModelEntry, ProviderId, Wire};
use porter_core::capability::{AgentProgram, AgentProtocol};
use porter_core::consent::{Usage, Verdict};
use porter_core::{AccountId, DataClass, Locality};
use porter_dbus::EndpointArg;
use porter_infer::ModelRef;
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::TcpListener;

/// The arguments of `OpenEndpoint`, as the bus carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    /// The agent program's id.
    pub program: String,
    /// The route.
    pub route: porter_dbus::RouteArg,
    /// The data class, as a slug.
    pub class: String,
    /// The protocols the program speaks, as slugs.
    pub protocols: Vec<String>,
}

/// Where an endpoint is and the secret to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The session id (`CloseEndpoint` takes it).
    pub session: String,
    /// `http`: agents only take http base URLs.
    pub scheme: &'static str,
    /// Always loopback.
    pub host: IpAddr,
    /// The ephemeral port.
    pub port: u16,
    /// The token the agent sends as its API key.
    pub token: Token,
    /// The protocol `base_url` is for.
    pub protocol: AgentProtocol,
}

impl Endpoint {
    /// The base URL to give the agent: for Anthropic's SDK the origin (it appends `/v1/messages`);
    /// for an OpenAI client the origin and `/v1`.
    pub fn base_url(&self) -> String {
        let origin = format!("{}://{}:{}", self.scheme, self.host, self.port);
        match self.protocol {
            AgentProtocol::AnthropicMessages => origin,
            _ => format!("{origin}/v1"),
        }
    }

    /// The bus reply: this is the one place the token goes on the bus.
    pub fn arg(&self) -> EndpointArg {
        (
            self.session.clone(),
            self.scheme.to_owned(),
            self.host.to_string(),
            self.port,
            self.base_url(),
            self.token.expose().to_owned(),
            crate::settings::slug_of(&self.protocol),
        )
    }
}

/// What a route can speak natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Speaks {
    Both,
    Only(AgentProtocol),
}

impl Speaks {
    fn takes(self, protocol: AgentProtocol) -> bool {
        match self {
            Speaks::Both => true,
            Speaks::Only(one) => one == protocol,
        }
    }
}

/// The wire a provider's catalogue reaches speak.
pub fn provider_wire(entries: &[ModelEntry], provider: &ProviderId) -> Option<Wire> {
    entries.iter().find_map(|entry| match &entry.locality {
        model_catalog::Locality::Remote { reach } => reach
            .iter()
            .find(|one| one.provider == *provider)
            .map(|one| one.wire),
        model_catalog::Locality::OnDevice => None,
    })
}

/// The reach of `provider` for the model id `named` there, with its entry.
pub fn priced<'a>(
    entries: &'a [ModelEntry],
    provider: &ProviderId,
    named: &str,
) -> Option<(&'a ModelEntry, &'a model_catalog::Reach)> {
    entries.iter().find_map(|entry| match &entry.locality {
        model_catalog::Locality::Remote { reach } => reach
            .iter()
            .find(|one| one.provider == *provider && one.model.0 == named)
            .map(|one| (entry, one)),
        model_catalog::Locality::OnDevice => None,
    })
}

/// The card of the model on this computer (or attached) whose catalogue id is `id`.
pub fn local_card(engines: &Engines, id: &str) -> Option<porter_infer::ModelCard> {
    engines
        .listed()
        .into_iter()
        .map(|one| one.card)
        .find(|card| card.model.as_str() == id && !matches!(card.locality, Locality::Cloud { .. }))
}

fn refused(cause: Cause, text: &str) -> Refusal {
    Refusal::new(cause, text)
}

impl Agents {
    /// Opens an endpoint for `owner` (the launcher's bus connection).
    pub async fn open(&self, owner: &str, request: OpenRequest) -> Result<Endpoint, Refusal> {
        let engines = self.engines().clone();
        let settings = engines.settings();
        if settings.agent_endpoint != AgentEndpoint::On {
            return Err(refused(
                Cause::EndpointOff,
                "agent endpoints are off (ai.agents.endpoint)",
            ));
        }
        let program = AgentProgram::parse(&request.program)
            .map_err(|_| Refusal::invalid("the program is not an agent program id"))?;
        let class: DataClass = crate::settings::from_slug(&request.class)
            .ok_or_else(|| Refusal::invalid("the class is not a data class"))?;
        let protocols: Vec<AgentProtocol> = request
            .protocols
            .iter()
            .map(|slug| {
                crate::settings::from_slug(slug)
                    .ok_or_else(|| Refusal::invalid("a protocol is not an agent protocol"))
            })
            .collect::<Result<_, _>>()?;
        let route = Route::parse(request.route)?;
        let app = agent_app(&program);
        let speaks = check_route(&engines, &route, &app, class).await?;
        let protocol = [
            AgentProtocol::AnthropicMessages,
            AgentProtocol::OpenAiCompatible,
        ]
        .into_iter()
        .find(|one| protocols.contains(one) && speaks.takes(*one))
        .ok_or_else(|| {
            refused(
                Cause::UnsupportedProtocol,
                "none of the program's protocols is one the route serves",
            )
        })?;
        let token = Token::fresh().ok_or_else(|| refused(Cause::Unavailable, "no randomness"))?;
        let session =
            random_hex::<8>().ok_or_else(|| refused(Cause::Unavailable, "no randomness"))?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| refused(Cause::Unavailable, "no loopback port"))?;
        let port = listener
            .local_addr()
            .map_err(|_| refused(Cause::Unavailable, "no loopback port"))?
            .port();
        let ctx = Ctx {
            program,
            app,
            route,
            class,
            token: token.clone(),
            engines,
            audit: self.audit(),
            clock: self.clock(),
        };
        self.start(session.clone(), owner, listener, ctx);
        Ok(Endpoint {
            session,
            scheme: "http",
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            token,
            protocol,
        })
    }
}

/// Checks that the route exists, is granted and may take data of `class`, and says what it speaks.
async fn check_route(
    engines: &Engines,
    route: &Route,
    app: &porter_core::AppId,
    class: DataClass,
) -> Result<Speaks, Refusal> {
    let settings = engines.settings();
    match &route.target {
        Target::Account(account) => {
            let cloud = engines
                .cloud()
                .ok_or_else(|| refused(Cause::NoSuchRoute, "this daemon has no hosted models"))?;
            if !admits(&settings, class, &Locality::Cloud { region: None }) {
                return Err(refused(
                    Cause::FloorRefused,
                    "the policy keeps data of this class off the cloud",
                ));
            }
            let provider = granted_provider(cloud, account, app, class).await?;
            let wire = provider_wire(cloud.entries(), &provider).ok_or_else(|| {
                refused(Cause::NoSuchRoute, "the account's provider has no models")
            })?;
            if let Models::Listed(ids) = &route.models {
                for id in ids {
                    if priced(cloud.entries(), &provider, id).is_none() {
                        return Err(Refusal::invalid(
                            "a listed model id is not one the account's provider prices",
                        ));
                    }
                }
            }
            Ok(match wire {
                Wire::AnthropicMessages => Speaks::Only(AgentProtocol::AnthropicMessages),
                Wire::OpenAiCompat => Speaks::Only(AgentProtocol::OpenAiCompatible),
            })
        }
        Target::Model(id) => {
            engines.look_at_attached().await;
            let card = local_card(engines, id)
                .ok_or_else(|| refused(Cause::NoSuchRoute, "no such model on this computer"))?;
            if !admits(&settings, class, &card.locality) {
                return Err(refused(
                    Cause::FloorRefused,
                    "the policy keeps data of this class off that model",
                ));
            }
            Ok(Speaks::Both)
        }
    }
}

/// The provider of `account` when `app` holds a grant on it for `class`.
pub async fn granted_provider(
    cloud: &crate::cloud::Cloud,
    account: &AccountId,
    app: &porter_core::AppId,
    class: DataClass,
) -> Result<ProviderId, Refusal> {
    let verdicts = cloud.accounts(app, class, Usage::Interactive).await;
    let one = verdicts
        .iter()
        .find(|one| one.account == *account)
        .ok_or_else(|| refused(Cause::NoSuchRoute, "accountd knows no such account"))?;
    if !matches!(one.verdict, Verdict::Granted { .. }) {
        return Err(refused(
            Cause::NotGranted,
            "the account is not granted to this program for this class",
        ));
    }
    provider_of(one, &known_providers(cloud.entries())).ok_or_else(|| {
        refused(
            Cause::NoSuchRoute,
            "the account is of no provider this build knows",
        )
    })
}

/// The local model a route names, as the router keys it.
pub fn model_ref_of(card: &porter_infer::ModelCard) -> ModelRef {
    ModelRef {
        account: card.account.clone(),
        model: card.model.clone(),
    }
}
