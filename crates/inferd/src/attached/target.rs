//! Where an attached engine is and how a request to it is signed: the endpoint of one connect.

use super::config::{Attached, Place, Reach};
use super::key::{KeyFile, KeyFileProblem};
use model_http::{AuthHeader, HostName, HttpEndpoint, HttpTarget, Proxy, Timeouts, UrlPath};
use porter_infer::ComputerName;
use std::path::PathBuf;

/// An attached engine as a connection is made to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// How it is reached.
    pub reach: Reach,
    /// Its bearer token's file, when it wants one.
    pub key: Option<KeyFile>,
    /// Where the data goes.
    pub place: Place,
    /// The machine it runs on, when the entry names one.
    pub computer: Option<ComputerName>,
}

impl Target {
    /// The target an entry names.
    pub fn of(attached: &Attached) -> Self {
        Self {
            reach: attached.reach.clone(),
            key: attached.key_file.clone().map(KeyFile::at),
            place: attached.place,
            computer: attached.computer.clone(),
        }
    }

    /// The socket, when the engine is reached over one.
    pub fn socket(&self) -> Option<&PathBuf> {
        match &self.reach {
            Reach::Socket(path) | Reach::Tailnet { socket: path, .. } => Some(path),
            Reach::Loopback { .. } => None,
        }
    }

    /// Whether the engine is a computer on the Tailscale network, reached through a relay that
    /// asks Tailscale and the computer before it answers: slower to answer than a socket the
    /// person's own tunnel holds open.
    pub fn is_relayed(&self) -> bool {
        self.reach.is_tailnet()
    }

    /// The endpoint of one connect: the key file is read now (so a token that was rotated is the
    /// one sent), and a file that is refused is why there is no endpoint.
    pub fn endpoint(&self, base: &str, timeouts: Timeouts) -> Result<HttpEndpoint, KeyFileProblem> {
        let auth = match &self.key {
            Some(key) => AuthHeader::Bearer(key.read()?),
            None => AuthHeader::None,
        };
        Ok(self.endpoint_with(auth, base, timeouts))
    }

    /// The endpoint with no token: what a turn sends when the key file is refused (the engine
    /// answers 401, and no token has gone anywhere).
    pub fn unsigned(&self, base: &str, timeouts: Timeouts) -> HttpEndpoint {
        self.endpoint_with(AuthHeader::None, base, timeouts)
    }

    fn endpoint_with(&self, auth: AuthHeader, base: &str, timeouts: Timeouts) -> HttpEndpoint {
        let target = match &self.reach {
            Reach::Socket(path) | Reach::Tailnet { socket: path, .. } => {
                HttpTarget::Unix(path.clone())
            }
            Reach::Loopback { host, port } => HttpTarget::Tcp {
                host: HostName(host.to_string()),
                port: *port,
            },
        };
        HttpEndpoint {
            target,
            proxy: Proxy::Direct,
            base: UrlPath(base.to_owned()),
            auth,
            headers: Vec::new(),
            timeouts,
        }
    }
}
