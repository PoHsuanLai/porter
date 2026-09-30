//! How an account's servers and capabilities are found.

use serde::{Deserialize, Serialize};

/// The `[discovery]` table: `kind = "..."`, plus `v = { ... }` for the kinds with data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Discovery {
    /// Mozilla-style autoconfig from the address's domain.
    Autoconfig,
    /// `.well-known` URLs (CalDAV, CardDAV, JMAP) and SRV records.
    WellKnown,
    /// The JMAP session resource.
    JmapSession,
    /// Nextcloud's OCS capabilities endpoint.
    NextcloudOcs,
    /// The endpoints in the file are all there is.
    Fixed,
    /// A local runtime probed on `127.0.0.1` at these ports (design/31 §3.3).
    ProbePorts {
        /// The ports, in probe order.
        ports: Vec<Port>,
    },
    /// The AI provider's model list (`/v1/models`, `models.list`).
    ModelList,
}

/// A TCP port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Port(pub u16);
