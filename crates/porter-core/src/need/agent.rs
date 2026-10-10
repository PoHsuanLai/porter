//! The need for an account that runs an agent program.

use crate::capability::{AgentProgram, AgentProtocol, Offered};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// "Which account runs this agent": the program, and what the caller needs of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentNeed {
    /// The program asked for; an account offers only the programs its provider file names.
    pub program: AgentProgram,
    /// Protocols it must speak (empty: any).
    pub protocols: BTreeSet<AgentProtocol>,
    /// A base-URL override, if `Present`: the route that puts inferd between the agent and the
    /// provider needs one.
    pub base_url: Offered,
}

impl AgentNeed {
    /// An account that runs `program`, speaking these protocols, with a base-URL override if
    /// `base_url` is `Present`.
    pub fn new(
        program: AgentProgram,
        protocols: BTreeSet<AgentProtocol>,
        base_url: Offered,
    ) -> Self {
        Self {
            program,
            protocols,
            base_url,
        }
    }
}
