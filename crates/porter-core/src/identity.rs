//! Who a connection's peer is, from facts its transport read about the peer's process: the
//! systemd scope it lives in and the sandbox it runs in (03 §2.9, C39). One function serves the
//! compositor's Wayland clients and the daemons' bus callers, so one process gets one identity.
//! Pure: each edge reads `/proc/<pid>/cgroup` and the sandbox's metadata and hands the text here.

use crate::app_id::{AppId, AppName, Isolation};
use crate::error::CoreError;
use crate::scope::{FLATPAK_LAUNCHER, ScopeName};
pub use peephole_core::CgroupPath;
use serde::{Deserialize, Serialize};

/// What the peer's sandbox says about it, read by the edge from the sandbox's own metadata
/// (Flatpak's `/.flatpak-info` for the pid) or from the `wp_security_context_v1` its listening
/// socket was made with. The text is as read; `identity_of` decides what it proves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SandboxFacts {
    /// No sandbox metadata: a native process.
    Native,
    /// A Flatpak sandbox's metadata.
    Flatpak {
        /// `[Application] name`.
        app: String,
        /// `[Instance] instance-id`.
        instance: String,
    },
    /// A security context's sandbox engine and what it says of the client.
    Engine {
        /// `sandbox_engine`, reverse-DNS (`org.flatpak`).
        engine: String,
        /// `app_id`.
        app: String,
        /// `instance_id`.
        instance: String,
    },
}

/// Everything an edge knows about a peer's process, read once where the connection arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerFacts {
    /// The process, from the socket's peer credentials.
    pub pid: u32,
    /// The process's cgroup.
    pub cgroup: CgroupPath,
    /// What its sandbox says, if it has one.
    pub sandbox: SandboxFacts,
}

/// Whether the facts prove which app the peer is. Policy trusts only a proven id; an unproven
/// peer is known at most by a `ClaimedId` and never gets an "Always" grant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PeerIdentity {
    /// The app, and how its name was established.
    Proven(AppId),
    /// Nothing proves a name.
    Unproven,
}

/// The security-context engine name Flatpak gives its sockets.
const FLATPAK_ENGINE: &str = "org.flatpak";

/// The app `facts` prove, if any. A sandbox's metadata wins: Flatpak's (by its file or its
/// security context) proves a `Flatpak` id when the process also sits in the scope Flatpak
/// makes for a run of that app (`app-flatpak-<id>-<n>.scope`, as peephole's `judge` asks), so a
/// security context a native process opened cannot name a Flatpak app by itself. Another engine
/// proves nothing, since `Isolation` has no variant for it. A native process is proven
/// `Unsandboxed` by a launcher's scope whose name is reverse-DNS; anything else,
/// `app-firefox-3.scope` included, is unproven (C39).
pub fn identity_of(facts: &PeerFacts) -> PeerIdentity {
    let proven = match &facts.sandbox {
        SandboxFacts::Flatpak { app, .. } => sandboxed(app, &facts.cgroup),
        SandboxFacts::Engine { engine, app, .. } if engine == FLATPAK_ENGINE => {
            sandboxed(app, &facts.cgroup)
        }
        SandboxFacts::Engine { .. } => None,
        SandboxFacts::Native => scope_app(&facts.cgroup).map(|name| AppId {
            name,
            isolation: Isolation::Unsandboxed,
        }),
    };
    proven.map_or(PeerIdentity::Unproven, PeerIdentity::Proven)
}

/// A Flatpak sandbox's app, when its name is one and `cgroup` is in Flatpak's scope for it.
fn sandboxed(app: &str, cgroup: &CgroupPath) -> Option<AppId> {
    let name = AppName::parse(app).ok()?;
    in_flatpak_scope(cgroup, app).then_some(AppId {
        name,
        isolation: Isolation::Flatpak,
    })
}

/// Whether a component of `cgroup` is the scope Flatpak makes for a run of `app`
/// (`app-flatpak-<id>-<n>.scope`, `n` decimal). The same rule as peephole-core's
/// `flatpak_scope_in`, here on the app id as the text the sandbox stated.
fn in_flatpak_scope(cgroup: &CgroupPath, app: &str) -> bool {
    cgroup
        .as_str()
        .split('/')
        .filter_map(ScopeName::parse)
        .any(|scope| scope.is_flatpak() && scope.app() == app)
}

/// The app a launcher's scope names ([`ScopeName`]). Flatpak's own scopes prove nothing by
/// name: a real Flatpak process brings sandbox facts.
fn scope_app(cgroup: &CgroupPath) -> Option<AppName> {
    let scope = ScopeName::parse(cgroup.leaf())?;
    if scope.launcher() == Some(FLATPAK_LAUNCHER) {
        return None;
    }
    AppName::parse(scope.app()).ok()
}

/// An app id a client declared itself (`xdg_toplevel.app_id`, `foot`, `firefox`): what an
/// unproven peer is known by. Any printable text of 1 to 255 bytes without `/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClaimedId(String);

impl ClaimedId {
    /// The id written as `text`.
    pub fn parse(text: &str) -> Result<ClaimedId, CoreError> {
        let printable = !text.chars().any(|c| c.is_control() || c == '/');
        if !text.is_empty() && text.len() <= 255 && printable {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "claimed id",
                text: text.to_owned(),
            })
        }
    }
}

porter_ids::string_newtype!(ClaimedId, CoreError);

#[cfg(test)]
mod tests;
