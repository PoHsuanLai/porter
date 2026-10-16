//! A unit in inferd's caller table, with the programs that may run in it.

use serde::{Deserialize, Serialize};

/// A unit under `[apps]` or `agent_launcher`: its name alone (`"memoryd.service"`, whose program
/// the shared table knows: a daemon's `libexec/quire/<name>`), or its name and the programs that
/// may run in it, for a unit the name does not give a program for (a scope, a dotted service):
///
/// ```toml
/// "org.quire.Companion" = [
///     "companiond.service",
///     { unit = "docket-acp.scope", exe = ["/usr/bin/docket-acp", "/opt/quire/releases/*/bin/docket-acp"] },
/// ]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Unit {
    /// A unit whose program the shared table gives by its name.
    Named(String),
    /// A unit and the programs (absolute paths, one `*` per segment) that may run in it.
    Running {
        /// The systemd unit.
        unit: String,
        /// The programs; empty is the same as [`Unit::Named`].
        exe: Vec<String>,
    },
}

impl Unit {
    /// The systemd unit.
    pub fn name(&self) -> &str {
        match self {
            Self::Named(unit) | Self::Running { unit, .. } => unit,
        }
    }

    /// The programs the entry names (none for a bare unit name).
    pub fn exe(&self) -> Vec<String> {
        match self {
            Self::Named(_) => Vec::new(),
            Self::Running { exe, .. } => exe.clone(),
        }
    }
}

impl From<String> for Unit {
    fn from(unit: String) -> Self {
        Self::Named(unit)
    }
}
