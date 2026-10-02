//! Who acted. Set by the transport or the daemon, never self-asserted in a request body.

use crate::ids::{ClientName, RunId, SessionId};
use porter_core::AppName;
use serde::{Deserialize, Serialize};

/// The party that did something: one type for the undo journal, the eventlog and the audit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Actor {
    /// The person, through this app (`org.quire.Shell` for the launcher).
    User {
        /// The app or shell they acted in.
        via: AppName,
    },
    /// The companion, in one session, in one role.
    Companion {
        /// The session.
        session: SessionId,
        /// Which of its agents.
        role: AgentRole,
    },
    /// An external MCP client.
    Mcp {
        /// How it named itself.
        client: ClientName,
    },
    /// An app acting on its own (sync, rules).
    App {
        /// The app.
        app: AppName,
    },
    /// A non-quire app, observed with consent.
    ThirdParty {
        /// The app.
        app: AppName,
        /// How the observation reached us.
        channel: Channel,
    },
    /// A part of the system itself.
    System {
        /// Which part.
        part: SystemPart,
    },
    /// A change nobody explained (a file that changed behind our back).
    Unknown,
}

/// Which companion agent acted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum AgentRole {
    /// The planner: sees trusted text only.
    Planner,
    /// The quarantined reader: sees untrusted content, acts on nothing.
    Reader,
    /// A computer-use run.
    Cua {
        /// The run.
        run: RunId,
    },
}

/// How a third-party app's activity was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// The accessibility bus.
    Atspi,
    /// The app's own D-Bus interface.
    Dbus,
    /// A desktop portal.
    Portal,
    /// MPRIS.
    Mpris,
    /// A watched file.
    FileWatch,
}

/// A part of the system that acts by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemPart {
    /// memoryd.
    Memory,
    /// The action router.
    Router,
    /// A file watcher.
    Watcher,
    /// Nightly consolidation.
    Consolidation,
    /// Retention sweeps.
    Retention,
    /// cuad (a run's own bookkeeping).
    Cua,
}

/// An actor without its details: what policy tables and filters key on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// [`Actor::User`].
    User,
    /// [`Actor::Companion`] as planner or reader.
    Companion,
    /// [`Actor::Companion`] as a computer-use run.
    Cua,
    /// [`Actor::Mcp`].
    Mcp,
    /// [`Actor::App`].
    App,
    /// [`Actor::ThirdParty`].
    ThirdParty,
    /// [`Actor::System`].
    System,
    /// [`Actor::Unknown`].
    Unknown,
}

impl Actor {
    /// The kind of this actor; a computer-use run is its own kind.
    pub fn kind(&self) -> ActorKind {
        match self {
            Actor::User { .. } => ActorKind::User,
            Actor::Companion {
                role: AgentRole::Cua { .. },
                ..
            } => ActorKind::Cua,
            Actor::Companion { .. } => ActorKind::Companion,
            Actor::Mcp { .. } => ActorKind::Mcp,
            Actor::App { .. } => ActorKind::App,
            Actor::ThirdParty { .. } => ActorKind::ThirdParty,
            Actor::System { .. } => ActorKind::System,
            Actor::Unknown => ActorKind::Unknown,
        }
    }
}
