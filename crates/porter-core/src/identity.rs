//! Who a connection's peer is, from facts its transport read about the peer's process: the
//! systemd scope it lives in and the sandbox it runs in (03 §2.9, C39). One function serves the
//! compositor's Wayland clients and the daemons' bus callers, so one process gets one identity.
//! Pure: each edge reads `/proc/<pid>/cgroup` and the sandbox's metadata and hands the text here.

use crate::app_id::{AppId, AppName, Isolation};
use crate::error::CoreError;
use serde::{Deserialize, Serialize};

/// A process's cgroup v2 path (`/user.slice/…/app.slice/app-org.quire.Mail-12.scope`), as the
/// unified hierarchy's `0::` line of `/proc/<pid>/cgroup` writes it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CgroupPath(String);

impl CgroupPath {
    /// The path `text`: absolute, on one line.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        if text.starts_with('/') && !text.contains(['\n', '\0']) {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "cgroup path",
                text: text.to_owned(),
            })
        }
    }

    /// The unified-hierarchy path in `contents`, the text of `/proc/<pid>/cgroup`. A system
    /// with only cgroup v1 hierarchies has no such line, and the contents are refused.
    pub fn from_proc_cgroup(contents: &str) -> Result<Self, CoreError> {
        match contents.lines().find_map(|line| line.strip_prefix("0::")) {
            Some(path) => Self::parse(path),
            None => Err(CoreError::MalformedId {
                what: "cgroup path",
                text: contents.to_owned(),
            }),
        }
    }

    /// The path's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last component: the unit (or the delegated child of one) the process sits in.
    fn leaf(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or_default()
    }
}

/// What the peer's sandbox says about it, read by the edge from the sandbox's own metadata
/// (Flatpak's `/.flatpak-info` for the pid) or from the `wp_security_context_v1` its listening
/// socket was made with. The text is as read; `identity_of` decides what it proves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
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
pub enum PeerIdentity {
    /// The app, and how its name was established.
    Proven(AppId),
    /// Nothing proves a name.
    Unproven,
}

/// The security-context engine name Flatpak gives its sockets.
const FLATPAK_ENGINE: &str = "org.flatpak";

/// The launcher tag Flatpak writes into its own scopes' names.
const FLATPAK_LAUNCHER: &str = "flatpak";

/// The app `facts` prove, if any. A sandbox's metadata wins: Flatpak's (by its file or its
/// security context) proves a `Flatpak` id. Another engine proves nothing, since `Isolation`
/// has no variant for it. A native process is proven `Unsandboxed` by a launcher's scope whose
/// name is reverse-DNS; anything else, `app-firefox-3.scope` included, is unproven (C39).
pub fn identity_of(facts: &PeerFacts) -> PeerIdentity {
    let proven = match &facts.sandbox {
        SandboxFacts::Flatpak { app, .. } => sandboxed(app),
        SandboxFacts::Engine { engine, app, .. } if engine == FLATPAK_ENGINE => sandboxed(app),
        SandboxFacts::Engine { .. } => None,
        SandboxFacts::Native => scope_app(&facts.cgroup).map(|name| AppId {
            name,
            isolation: Isolation::Unsandboxed,
        }),
    };
    proven.map_or(PeerIdentity::Unproven, PeerIdentity::Proven)
}

/// A Flatpak sandbox's app, when its name is one.
fn sandboxed(app: &str) -> Option<AppId> {
    AppName::parse(app).ok().map(|name| AppId {
        name,
        isolation: Isolation::Flatpak,
    })
}

/// The app a launcher's scope names: `app-[<launcher>-]<id>-<random>.scope`, the id escaped as
/// a unit name component (XDG "Desktop Entry / systemd integration"). A literal `-` in an id is
/// written `\x2d`, so the dashes split the name unambiguously; a launcher tag is one plain word.
/// Flatpak's own scopes prove nothing by name: a real Flatpak process brings sandbox facts.
fn scope_app(cgroup: &CgroupPath) -> Option<AppName> {
    let unit = cgroup.leaf().strip_prefix("app-")?.strip_suffix(".scope")?;
    let parts: Vec<&str> = unit.split('-').collect();
    let (id, random) = match parts.as_slice() {
        [id, random] => (*id, *random),
        [launcher, id, random] if is_word(launcher) && *launcher != FLATPAK_LAUNCHER => {
            (*id, *random)
        }
        _ => return None,
    };
    if !is_word(random) {
        return None;
    }
    AppName::parse(&unescape(id)?).ok()
}

/// Whether `text` is one non-empty run of ASCII letters and digits.
fn is_word(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// A unit name component with its `\xHH` escapes undone, or `None` for a broken escape.
fn unescape(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        rest = match (first, tail) {
            (b'\\', [b'x', high, low, after @ ..]) => {
                bytes.push((hex_digit(*high)? << 4) | hex_digit(*low)?);
                after
            }
            (b'\\', _) => return None,
            _ => {
                bytes.push(first);
                tail
            }
        };
    }
    String::from_utf8(bytes).ok()
}

/// The value of one hexadecimal digit.
fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
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

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ClaimedId {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<ClaimedId> for String {
    fn from(id: ClaimedId) -> String {
        id.0
    }
}

#[cfg(test)]
mod tests;
