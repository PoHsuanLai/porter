//! Who a message is between: the roster's agent reference and an address (agent plus Space).

use crate::actor::{Actor, AgentRole};
use crate::ids::{RunId, TaskId};
use porter_core::SpaceId;
use serde::{Deserialize, Serialize};

/// One party of the companion's roster: the front agent, a worker task, a computer-use run, or
/// the person. It names a party; it grants nothing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum AgentRef {
    /// The one persistent companion identity (the front thread; its tasks are its sessions).
    Companion,
    /// A worker subagent, by the task it runs as.
    Worker {
        /// The task.
        task: TaskId,
    },
    /// A computer-use run.
    Cua {
        /// The run.
        run: RunId,
    },
    /// The person. They are a sender like any other; their words are `Trusted` only because the
    /// message's label says so.
    User,
}

impl AgentRef {
    /// The roster party an actor is, if it is one: the person, the planner or reader (both are
    /// the companion), a worker, or a run. Apps, MCP clients, ACP agents, the command line and the system are not on the
    /// roster.
    pub fn of(actor: &Actor) -> Option<AgentRef> {
        match actor {
            Actor::User { .. } => Some(AgentRef::User),
            Actor::Companion { role, .. } => Some(match role {
                AgentRole::Planner | AgentRole::Reader => AgentRef::Companion,
                AgentRole::Worker { task } => AgentRef::Worker { task: task.clone() },
                AgentRole::Cua { run } => AgentRef::Cua { run: run.clone() },
            }),
            Actor::Mcp { .. }
            | Actor::Acp { .. }
            | Actor::Cli
            | Actor::App { .. }
            | Actor::ThirdParty { .. }
            | Actor::System { .. }
            | Actor::Unknown => None,
        }
    }
}

/// An agent in a Space: one end of a message. A message may cross Spaces, so both ends keep
/// theirs; the person's end carries the Space they were working in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Address {
    /// Who.
    pub agent: AgentRef,
    /// In which Space.
    pub space: SpaceId,
}

/// Whether a message stays in one Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Crossing {
    /// Both ends are in the same Space.
    Within,
    /// The ends are in different Spaces. The message still grants no memory read in either.
    Across,
}

impl Address {
    /// `agent` in `space`.
    pub fn new(agent: AgentRef, space: SpaceId) -> Self {
        Self { agent, space }
    }

    /// Whether a message from `self` to `to` crosses Spaces.
    pub fn crossing(&self, to: &Address) -> Crossing {
        if self.space == to.space {
            Crossing::Within
        } else {
            Crossing::Across
        }
    }
}
