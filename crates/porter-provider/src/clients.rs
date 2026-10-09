//! The OAuth client registry's file format (porter PLAN §2.6): which client id this build
//! presents to each issuer, per build channel. The ids shipped with the desktop live in
//! `/usr/share/porter/clients.toml`, a person's own in `$XDG_CONFIG_HOME/porter/clients.toml`
//! (written only by Settings). The registry itself, which layers the two and renews with them,
//! is `porter-oauth`; a client id is never part of an account.
//!
//! ```toml
//! [[client]]
//! issuer = "microsoft"
//! channel = "stable"
//! client_id = "00000000-0000-0000-0000-000000000000"
//! ```

use crate::spec::{Issuer, IssuerEndpoints};
use porter_core::SecretText;
use serde::{Deserialize, Serialize};

/// An OAuth client id. A public client cannot keep one secret (that is why there is PKCE), so
/// its `Debug` is plain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub String);

/// Which build of the desktop presents a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientChannel {
    /// A release.
    Stable,
    /// A pre-release.
    Beta,
    /// A development build.
    Development,
}

/// One registered client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientEntry {
    /// The issuer it is registered with.
    pub issuer: Issuer,
    /// The build channel that uses it.
    pub channel: ClientChannel,
    /// Its id.
    pub client_id: ClientId,
    /// The application secret an issuer requires of every installed client (Google's desktop
    /// clients); not the user's credential, and identical for every user of the build. Absent
    /// for public clients (Microsoft).
    #[serde(default)]
    pub client_secret: Option<SecretText>,
    /// Endpoints other than the ones the issuer publishes.
    #[serde(default)]
    pub endpoints: Option<IssuerEndpoints>,
}

/// A `clients.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClientsFile {
    /// Its rows.
    #[serde(rename = "client", default)]
    pub clients: Vec<ClientEntry>,
}

/// Why a clients file was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClientsFileError {
    /// Not valid TOML, or not the clients schema.
    #[error("clients file: {0}")]
    Syntax(String),
    /// Two rows for one issuer and channel: which one applies would be a guess.
    #[error("clients file: {0:?} {1:?} is listed twice")]
    Duplicate(Issuer, ClientChannel),
}

impl ClientEntry {
    /// Whether the row's own `endpoints` leave its issuer's servers: some endpoint's origin
    /// (scheme, host, port) is none of the origins the issuer publishes. A path may differ
    /// (Microsoft's `organizations` or `consumers` tenant). A row with no endpoints stays.
    pub fn leaves_the_issuer(&self) -> bool {
        let origins = |e: &IssuerEndpoints| -> Vec<porter_core::Origin> {
            [
                Some(&e.authorize),
                Some(&e.token),
                e.revoke.as_ref(),
                e.device.as_ref(),
            ]
            .into_iter()
            .flatten()
            .map(porter_core::EndpointUrl::origin)
            .collect()
        };
        let published = origins(&self.issuer.endpoints());
        self.endpoints
            .as_ref()
            .is_some_and(|own| origins(own).iter().any(|o| !published.contains(o)))
    }
}

/// The clients one file's text declares, or why it is refused.
pub fn parse_clients(text: &str) -> Result<ClientsFile, ClientsFileError> {
    let file: ClientsFile =
        toml::from_str(text).map_err(|e| ClientsFileError::Syntax(e.to_string()))?;
    let mut seen: Vec<(Issuer, ClientChannel)> = Vec::new();
    for entry in &file.clients {
        let key = (entry.issuer, entry.channel);
        if seen.contains(&key) {
            return Err(ClientsFileError::Duplicate(key.0, key.1));
        }
        seen.push(key);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
[[client]]
issuer = "microsoft"
channel = "stable"
client_id = "ms-stable"

[[client]]
issuer = "google"
channel = "stable"
client_id = "g-stable"
client_secret = "app-secret"

[[client]]
issuer = "microsoft"
channel = "development"
client_id = "ms-dev"
"#;

    #[test]
    fn a_clients_file_parses_to_its_rows() {
        let file = parse_clients(FILE).expect("parses");
        let rows: Vec<_> = file
            .clients
            .iter()
            .map(|c| {
                (
                    c.issuer,
                    c.channel,
                    c.client_id.0.as_str(),
                    c.client_secret.is_some(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (Issuer::Microsoft, ClientChannel::Stable, "ms-stable", false),
                (Issuer::Google, ClientChannel::Stable, "g-stable", true),
                (
                    Issuer::Microsoft,
                    ClientChannel::Development,
                    "ms-dev",
                    false
                ),
            ]
        );
        let again = toml::to_string(&file).expect("toml");
        assert_eq!(parse_clients(&again), Ok(file));
    }

    #[test]
    fn a_bad_clients_file_is_refused() {
        let twice = format!(
            "{FILE}\n[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"other\"\n"
        );
        assert_eq!(
            parse_clients(&twice),
            Err(ClientsFileError::Duplicate(
                Issuer::Google,
                ClientChannel::Stable
            ))
        );
        assert!(matches!(
            parse_clients("[[client]]\nissuer = \"nowhere\""),
            Err(ClientsFileError::Syntax(_))
        ));
        assert_eq!(parse_clients(""), Ok(ClientsFile::default()));
    }

    #[test]
    fn the_application_secret_never_shows_in_debug() {
        let file = parse_clients(FILE).expect("parses");
        assert!(!format!("{file:?}").contains("app-secret"));
    }
}
