//! Where a session's requests go: an API-key account at a provider, or a model on this computer
//! (a supervised engine, a runtime the person runs, an engine they attached). The route also says
//! which model ids the agent may name; an id it does not serve is an error, never a substitute.

use super::refusal::Refusal;
use crate::settings::Settings;
use porter_core::capability::AgentProgram;
use porter_core::{AccountId, AppId, AppName, DataClass, Isolation, Locality};
use porter_dbus::{MODELS_ANY, MODELS_LISTED, ROUTE_ACCOUNT, ROUTE_MODEL, RouteArg};
use porter_infer::LocalOnly;

/// What the route is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An API-key account, forwarded to its provider with the account's key.
    Account(AccountId),
    /// A model on this computer or an attached engine, by catalogue id.
    Model(String),
}

/// Which ids the agent may name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Models {
    /// Exactly these. An account route forwards the id as named; a model route answers each
    /// with its own model.
    Listed(Vec<String>),
    /// Whatever the agent names (a typed route option, not a default): an account route
    /// forwards it as named, a model route answers it with its own model.
    Any,
}

/// A session's route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Where requests go.
    pub target: Target,
    /// Which ids may be named.
    pub models: Models,
}

impl Route {
    /// The route a bus argument names.
    pub fn parse(arg: RouteArg) -> Result<Self, Refusal> {
        let (kind, id, mode, listed) = arg;
        let target = match kind.as_str() {
            ROUTE_ACCOUNT => Target::Account(
                AccountId::parse(&id)
                    .map_err(|_| Refusal::invalid("the route's account id is not an id"))?,
            ),
            ROUTE_MODEL if !id.is_empty() => Target::Model(id),
            ROUTE_MODEL => return Err(Refusal::invalid("the route's model id is empty")),
            other => {
                return Err(Refusal::invalid(format!(
                    "a route of kind {other}: {ROUTE_ACCOUNT} or {ROUTE_MODEL}"
                )));
            }
        };
        let models = match mode.as_str() {
            MODELS_ANY if listed.is_empty() => Models::Any,
            MODELS_ANY => return Err(Refusal::invalid("an any-model route lists no ids")),
            MODELS_LISTED => {
                let mut ids = listed;
                if ids.is_empty() {
                    match &target {
                        Target::Model(own) => ids.push(own.clone()),
                        Target::Account(_) => {
                            return Err(Refusal::invalid(
                                "a listed account route names at least one model id",
                            ));
                        }
                    }
                }
                if ids.iter().any(String::is_empty) {
                    return Err(Refusal::invalid("a model id is empty"));
                }
                Models::Listed(ids)
            }
            other => {
                return Err(Refusal::invalid(format!(
                    "models of {other}: {MODELS_LISTED} or {MODELS_ANY}"
                )));
            }
        };
        Ok(Self { target, models })
    }

    /// Whether the agent may name `id`.
    pub fn serves(&self, id: &str) -> bool {
        match &self.models {
            Models::Any => true,
            Models::Listed(ids) => ids.iter().any(|one| one == id),
        }
    }
}

/// The app a program's grants and spend are held under: `org.quire.Agent.<program>`. The person
/// grants an account to the program with `accountd add <provider> --allow
/// org.quire.Agent.claude-code`; the launcher is the one that may ask inferd to act for it.
pub fn agent_app(program: &AgentProgram) -> AppId {
    // A program is a lowercase letter then lowercase letters, digits and `-`: always one element.
    AppId {
        name: AppName::parse(&format!("org.quire.Agent.{}", program.as_str()))
            .expect("a program is a valid app name element"),
        isolation: Isolation::Unsandboxed,
    }
}

/// Whether data of `class` may go to `locality` under the settings in force: a cloud account
/// only when `ai.local_only` is off and the class's floor says anywhere; another machine of the
/// person's only as the floor (and `ai.attached.my_network`) allow; this computer always.
pub fn admits(settings: &Settings, class: DataClass, locality: &Locality) -> bool {
    let cloud_ok =
        !matches!(locality, Locality::Cloud { .. }) || settings.policy.local_only == LocalOnly::Off;
    cloud_ok && settings.routing_policy().floor(class).admits(locality)
}

#[cfg(test)]
mod tests;
