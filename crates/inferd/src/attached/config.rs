//! `[engines.attached."<catalog id>"]`: what the file says of an engine the person already runs,
//! and the checks that need nothing but the file (the target is a Unix socket or a loopback
//! address, the data's place is named). The catalogue and the file system are asked later
//! (`model`, `key`).

use crate::startup::SUN_PATH;
use model_http::Port;
use porter_core::{Locality, ModelId};
use porter_infer::ComputerName;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Where an attached engine's data goes: written for every entry, with no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Place {
    /// The engine runs on this computer (a server of the person's own, a container).
    ThisDevice,
    /// The engine runs on another machine of the person's, reached through a tunnel: the data
    /// leaves this computer.
    MyNetwork,
}

impl Place {
    /// Where routing counts the engine to be.
    pub fn locality(self) -> Locality {
        match self {
            Place::ThisDevice => Locality::OnDevice,
            Place::MyNetwork => Locality::LocalNetwork,
        }
    }
}

/// One `[engines.attached."<id>"]` table, as written. Everything is optional here so that what is
/// missing is a typed refusal ([`AttachedError`]) and not a parser's sentence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachedEntry {
    /// A Unix socket the engine speaks HTTP over (an SSH `-L` to a local socket).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<PathBuf>,
    /// `http://127.0.0.1:<port>`: loopback only, plain HTTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// A file holding the bearer token (vLLM's `--api-key`): read at each connect, never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<PathBuf>,
    /// Where the data goes.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub place: Option<Place>,
    /// The name of the machine the engine runs on, so that several engines on one machine are one
    /// place (`computer:<name>`). Optional; an engine on another machine of yours that names none
    /// belongs to the computer called `other-computer` ([`ComputerName::other`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer: Option<String>,
}

/// How an attached engine is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// HTTP over this Unix socket.
    Socket(PathBuf),
    /// Plain HTTP to this loopback address.
    Loopback {
        /// Within `127.0.0.0/8`.
        host: Ipv4Addr,
        /// The port.
        port: Port,
    },
}

/// An entry that passed the checks of the file alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    /// The catalogue id of the model the engine serves.
    pub id: ModelId,
    /// How it is reached.
    pub reach: Reach,
    /// Where its bearer token is, when it wants one.
    pub key_file: Option<PathBuf>,
    /// Where the data goes.
    pub place: Place,
    /// The machine it runs on, when the entry names one.
    pub computer: Option<ComputerName>,
}

/// Why an attached engine is refused when the configuration loads.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachedError {
    /// The table's name is not a model id.
    #[error("engines.attached.{id}: not a model id")]
    BadId {
        /// The name.
        id: String,
    },
    /// `where` is missing: where the data goes is never assumed.
    #[error("engines.attached.{id}: `where` is required (\"this-device\" or \"my-network\")")]
    MissingWhere {
        /// The entry.
        id: String,
    },
    /// Neither `socket` nor `url`.
    #[error("engines.attached.{id}: name a `socket` or a `url`")]
    NoTarget {
        /// The entry.
        id: String,
    },
    /// Both `socket` and `url`.
    #[error("engines.attached.{id}: name a `socket` or a `url`, not both")]
    BothTargets {
        /// The entry.
        id: String,
    },
    /// A socket path that is not absolute.
    #[error("engines.attached.{id}: the socket path {path} is not absolute")]
    RelativeSocket {
        /// The entry.
        id: String,
        /// The path.
        path: String,
    },
    /// A socket path too long to connect to.
    #[error("engines.attached.{id}: the socket path is {len} bytes; it must be under 108")]
    SocketPathTooLong {
        /// The entry.
        id: String,
        /// Its length.
        len: usize,
    },
    /// A key file path that is not absolute.
    #[error("engines.attached.{id}: the key_file path {path} is not absolute")]
    RelativeKeyFile {
        /// The entry.
        id: String,
        /// The path.
        path: String,
    },
    /// A `url` that is not `http://<host>:<port>`.
    #[error("engines.attached.{id}: `url` must be http://127.0.0.1:<port>")]
    BadUrl {
        /// The entry.
        id: String,
    },
    /// An `https://` `url`: nothing here speaks TLS.
    #[error("engines.attached.{id}: `url` must be plain http (no TLS here); got https")]
    NotPlainHttp {
        /// The entry.
        id: String,
    },
    /// A host that is not loopback.
    #[error(
        "engines.attached.{id}: `url` host {host} is not loopback (127.0.0.0/8 or localhost); \
         reach another machine through a tunnel to a local socket or port"
    )]
    NotLoopback {
        /// The entry.
        id: String,
        /// The host as written.
        host: String,
    },
    /// A `computer` that is not a name (lowercase letters, digits, dots, dashes and underscores).
    #[error("engines.attached.{id}: `computer` must be a short lowercase name; got {name:?}")]
    BadComputer {
        /// The entry.
        id: String,
        /// The name as written.
        name: String,
    },
    /// An id the catalogue does not hold.
    #[error("engines.attached.{id}: no model of that id in the catalogue")]
    NotInCatalogue {
        /// The entry.
        id: String,
    },
    /// A catalogue entry with nothing a session can use.
    #[error("engines.attached.{id}: the catalogue entry offers no capability")]
    NoCapability {
        /// The entry.
        id: String,
    },
    /// A catalogue entry that inferd launches itself: only an entry whose serving is `attached`
    /// can be attached.
    #[error(
        "engines.attached.{id}: the catalogue entry is not one served by an engine of yours (serving is not attached)"
    )]
    NotAttachable {
        /// The entry.
        id: String,
    },
    /// A catalogue entry whose engine does not speak OpenAI-compatible chat.
    #[error("engines.attached.{id}: the catalogue entry has no OpenAI-compatible engine profile")]
    NoChatEngine {
        /// The entry.
        id: String,
    },
}

impl AttachedEntry {
    /// The entry named `name` as the checks of the file alone see it.
    pub fn check(&self, name: &str) -> Result<Attached, AttachedError> {
        let id = ModelId::parse(name).map_err(|_| AttachedError::BadId {
            id: name.to_owned(),
        })?;
        let here = || name.to_owned();
        let place = self
            .place
            .ok_or_else(|| AttachedError::MissingWhere { id: here() })?;
        let reach = match (&self.socket, &self.url) {
            (None, None) => return Err(AttachedError::NoTarget { id: here() }),
            (Some(_), Some(_)) => return Err(AttachedError::BothTargets { id: here() }),
            (Some(socket), None) => socket_reach(name, socket)?,
            (None, Some(url)) => url_reach(name, url)?,
        };
        if let Some(key) = self.key_file.as_ref().filter(|key| !key.is_absolute()) {
            return Err(AttachedError::RelativeKeyFile {
                id: here(),
                path: key.display().to_string(),
            });
        }
        let computer = match &self.computer {
            None => None,
            Some(text) => {
                Some(
                    ComputerName::parse(text).map_err(|_| AttachedError::BadComputer {
                        id: here(),
                        name: text.clone(),
                    })?,
                )
            }
        };
        Ok(Attached {
            id,
            reach,
            key_file: self.key_file.clone(),
            place,
            computer,
        })
    }
}

fn socket_reach(name: &str, socket: &Path) -> Result<Reach, AttachedError> {
    if !socket.is_absolute() {
        return Err(AttachedError::RelativeSocket {
            id: name.to_owned(),
            path: socket.display().to_string(),
        });
    }
    let len = socket.as_os_str().as_bytes().len();
    if len >= SUN_PATH {
        return Err(AttachedError::SocketPathTooLong {
            id: name.to_owned(),
            len,
        });
    }
    Ok(Reach::Socket(socket.to_path_buf()))
}

/// `http://<loopback host>:<port>`, a trailing slash allowed, nothing else.
pub fn url_reach(name: &str, url: &str) -> Result<Reach, AttachedError> {
    let bad = || AttachedError::BadUrl {
        id: name.to_owned(),
    };
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Err(AttachedError::NotPlainHttp {
            id: name.to_owned(),
        });
    }
    let rest = lower.strip_prefix("http://").ok_or_else(bad)?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.contains(['/', '?', '#', '@', ' ']) {
        return Err(bad());
    }
    let (host, port) = authority.rsplit_once(':').ok_or_else(bad)?;
    let port: u16 = port.parse().map_err(|_| bad())?;
    if port == 0 {
        return Err(bad());
    }
    let host = match host {
        "localhost" => Ipv4Addr::LOCALHOST,
        literal => match literal.parse::<Ipv4Addr>() {
            Ok(address) if address.is_loopback() => address,
            _ => {
                return Err(AttachedError::NotLoopback {
                    id: name.to_owned(),
                    host: host.to_owned(),
                });
            }
        },
    };
    Ok(Reach::Loopback {
        host,
        port: Port(port),
    })
}

#[cfg(test)]
mod tests;
