//! `rig.json`: what `porter-rig-servers` writes once everything it was asked for is listening,
//! and what a scenario reads to learn addresses and secrets. Fields of a fake that was not
//! started are absent. Written whole under a temporary name and renamed, so a reader that finds
//! the file finds all of it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name in the scratch directory.
pub const FILE: &str = "rig.json";

/// A mail server (IMAP, SMTP or POP3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailAt {
    /// Always `127.0.0.1`.
    pub host: String,
    /// The port, never below 1024.
    pub port: u16,
    /// `implicit`, `start_tls` or `plain` (porter-core's `Tls`, snake case).
    pub tls: String,
    /// The login name.
    pub user: String,
    /// The password (planted).
    pub password: String,
}

/// An HTTP fake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpAt {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// Its port.
    pub port: u16,
    /// The login name, for the fakes that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The password or app password (planted), for the fakes that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// The OAuth issuer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OauthAt {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// Its port.
    pub port: u16,
    /// `<url>/authorize`.
    pub authorize: String,
    /// `<url>/token`.
    pub token: String,
    /// `<url>/revoke`.
    pub revoke: String,
    /// `<url>/device`.
    pub device: String,
    /// The client the planted tokens belong to.
    pub client_id: String,
    /// The scope the planted refresh token carries.
    pub scope: String,
    /// A live refresh token (planted).
    pub refresh_token: String,
    /// A live access token (planted).
    pub access_token: String,
}

/// The LLM API fake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmApiAt {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// Its port.
    pub port: u16,
    /// The API root (the company's version prefix included).
    pub api_url: String,
    /// The wire: `bearer`.
    pub auth: String,
    /// The live key (planted).
    pub api_key: String,
}

/// The fake Ollama.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaAt {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// Its port: the same after a stop and a start.
    pub port: u16,
    /// The names of the models it lists.
    pub models: Vec<String>,
}

/// Everything the rig started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigFile {
    /// The scratch directory.
    pub dir: PathBuf,
    /// The rig's process id: what a scenario stops with SIGTERM.
    pub pid: u32,
    /// The scratch CA, PEM, the one root a client of the TLS fakes trusts.
    pub ca: PathBuf,
    /// The control endpoint's URL (`http://127.0.0.1:<port>`).
    pub control: String,
    /// Every planted secret, for a scan.
    pub secrets: Vec<String>,
    /// IMAP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imap: Option<MailAt>,
    /// SMTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp: Option<MailAt>,
    /// POP3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pop3: Option<MailAt>,
    /// A plain DAV server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dav: Option<HttpAt>,
    /// A Nextcloud.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nextcloud: Option<HttpAt>,
    /// The OAuth issuer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OauthAt>,
    /// The Graph drive (it accepts the issuer's access tokens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<HttpAt>,
    /// The fake Ollama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaAt>,
    /// The LLM API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_api: Option<LlmApiAt>,
}

impl RigFile {
    /// The path of the file under `dir`.
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// Writes the file under `dir` whole.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let temp = dir.join(format!(".{FILE}.tmp"));
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, Self::path(dir))
    }

    /// Reads the file under `dir`.
    pub fn read(dir: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(Self::path(dir))?;
        serde_json::from_str(&text).map_err(std::io::Error::other)
    }
}
