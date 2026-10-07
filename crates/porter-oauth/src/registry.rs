//! Which client id this build presents to an issuer: the ones shipped, with a person's own
//! (written only by Settings) laid over them.
//!
//! Nothing here finds a file: the caller passes the paths (`/usr/share/porter/clients.toml`,
//! `$XDG_CONFIG_HOME/porter/clients.toml`), so a test never reads the machine's. The Microsoft
//! client is ours and is shipped in the first file; no Google client id is shipped (the owner
//! registers one, docs/google.md), and a person may register their own. The mailo migration reads mailo's
//! `OAuthRegistry` (`oauth.json`, `~/mailo/crates/mail-runtime/src/signin.rs`) so E4 can move
//! `saved_clients` into the person's own file.

use porter_core::{EndpointUrl, SecretText};
use porter_provider::{
    ClientChannel, ClientEntry, ClientId, ClientsFile, ClientsFileError, Issuer, IssuerEndpoints,
    parse_clients,
};
use serde::Deserialize;
use std::path::Path;

/// Whether a client may be used for mail.
///
/// Gmail's IMAP and SMTP need the restricted scope `https://mail.google.com/`, which Google
/// grants a client only after a security assessment. A person's own client may use it (it is
/// their own project, in testing mode, for their own addresses), so a row says `byo = true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MailRights {
    /// Not a client of the person's own: mail's restricted scope is not asked.
    #[default]
    Withheld,
    /// The person's own client: mail may be asked.
    Byo,
}

/// Whether the client's consent screen is verified.
///
/// An app Google has not verified is in *testing*: only its test users may sign in, and Google
/// ends their sign-ins after seven days (design/31 R5). A row says `testing = true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppReview {
    /// Verified, or not an issuer that has the notion: sign-ins last until revoked.
    #[default]
    Verified,
    /// In testing: sign-ins last seven days.
    Testing,
}

/// What a client row says besides its id, secret and endpoints. [`porter_provider::ClientEntry`]
/// does not carry these (it is shared with mailo, which builds it field by field), so the
/// registry reads them from the same files beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ClientTraits {
    /// Whether mail may be asked.
    pub mail: MailRights,
    /// Whether the app is in testing.
    pub review: AppReview,
}

/// One row's traits, keyed as the row is.
type TraitRow = (Issuer, ClientChannel, ClientTraits);

/// The clients of this build, shipped and the person's own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientRegistry {
    shipped: Vec<ClientEntry>,
    own: Vec<ClientEntry>,
    shipped_traits: Vec<TraitRow>,
    own_traits: Vec<TraitRow>,
}

impl ClientRegistry {
    /// The shipped clients with the person's own laid over them. Every row has the default
    /// [`ClientTraits`]; [`ClientRegistry::with_traits`] and [`ClientRegistry::from_paths`] set them.
    pub fn layered(shipped: ClientsFile, own: ClientsFile) -> Self {
        Self {
            shipped: shipped.clients,
            own: own.clients,
            shipped_traits: Vec::new(),
            own_traits: Vec::new(),
        }
    }

    /// The registry from the two files at the paths the caller names. A file that does not exist
    /// is empty (a fresh install has no override); one that exists and is damaged is an error,
    /// never read as empty: that would report a configured client as missing.
    pub fn from_paths(shipped: &Path, own: &Path) -> Result<Self, RegistryError> {
        let (shipped_file, shipped_traits) = read_clients(shipped)?;
        let (own_file, own_traits) = read_clients(own)?;
        Ok(Self {
            shipped_traits,
            own_traits,
            ..Self::layered(shipped_file, own_file)
        })
    }

    /// The same registry, the row [`ClientRegistry::lookup`] finds for `issuer` on `channel`
    /// having `traits` (nothing changes when there is no such row).
    pub fn with_traits(
        mut self,
        issuer: Issuer,
        channel: ClientChannel,
        traits: ClientTraits,
    ) -> Self {
        let held = |entries: &[ClientEntry]| {
            entries
                .iter()
                .any(|c| c.issuer == issuer && c.channel == channel)
        };
        let layer = match held(&self.own) {
            true => &mut self.own_traits,
            false if held(&self.shipped) => &mut self.shipped_traits,
            false => return self,
        };
        layer.retain(|(i, c, _)| !(*i == issuer && *c == channel));
        layer.push((issuer, channel, traits));
        self
    }

    /// What the row [`ClientRegistry::lookup`] finds for `issuer` on `channel` says besides its
    /// id: the person's own row's if they have one, else the shipped row's. A client with
    /// nothing said has the defaults (no mail, verified).
    pub fn traits(&self, issuer: Issuer, channel: ClientChannel) -> ClientTraits {
        let row = |entries: &[ClientEntry], traits: &[TraitRow]| {
            entries
                .iter()
                .any(|c| c.issuer == issuer && c.channel == channel)
                .then(|| {
                    traits
                        .iter()
                        .find(|(i, c, _)| *i == issuer && *c == channel)
                        .map(|(_, _, t)| *t)
                        .unwrap_or_default()
                })
        };
        row(&self.own, &self.own_traits)
            .or_else(|| row(&self.shipped, &self.shipped_traits))
            .unwrap_or_default()
    }

    /// The client for `issuer` on `channel`: the person's own if they registered one, else the
    /// shipped one.
    pub fn lookup(&self, issuer: Issuer, channel: ClientChannel) -> Option<&ClientEntry> {
        self.own
            .iter()
            .chain(&self.shipped)
            .find(|c| c.issuer == issuer && c.channel == channel)
    }
}

/// The endpoints to use with `client`: its own override, else the issuer's published ones.
pub fn endpoints_of(client: &ClientEntry) -> IssuerEndpoints {
    client
        .endpoints
        .clone()
        .unwrap_or_else(|| client.issuer.endpoints())
}

/// Why a clients file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The file exists and could not be read.
    #[error("{path}: {reason}")]
    Unreadable {
        /// The file.
        path: String,
        /// What went wrong.
        reason: String,
    },
    /// The file is not a valid clients file.
    #[error("{path}: {source}")]
    Invalid {
        /// The file.
        path: String,
        /// What is wrong with it.
        source: ClientsFileError,
    },
    /// A mailo `oauth.json` that is not mailo's format.
    #[error("mailo oauth.json: {0}")]
    Mailo(String),
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(rename = "client", default)]
    clients: Vec<RawRow>,
}

#[derive(Deserialize)]
struct RawRow {
    issuer: Issuer,
    channel: ClientChannel,
    #[serde(default)]
    byo: bool,
    #[serde(default)]
    testing: bool,
}

/// The traits a clients file's rows say (`byo`, `testing`); the file was already parsed as a
/// clients file, so it is valid TOML of the clients schema.
fn traits_of(text: &str) -> Vec<TraitRow> {
    toml::from_str::<RawFile>(text)
        .map(|file| {
            file.clients
                .into_iter()
                .map(|row| {
                    let traits = ClientTraits {
                        mail: if row.byo {
                            MailRights::Byo
                        } else {
                            MailRights::Withheld
                        },
                        review: if row.testing {
                            AppReview::Testing
                        } else {
                            AppReview::Verified
                        },
                    };
                    (row.issuer, row.channel, traits)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_clients(path: &Path) -> Result<(ClientsFile, Vec<TraitRow>), RegistryError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((ClientsFile::default(), Vec::new()));
        }
        Err(e) => {
            return Err(RegistryError::Unreadable {
                path: path.display().to_string(),
                reason: e.to_string(),
            });
        }
    };
    let file = parse_clients(&text).map_err(|source| RegistryError::Invalid {
        path: path.display().to_string(),
        source,
    })?;
    Ok((file, traits_of(&text)))
}

/// A clients file as TOML, for the person's own file.
pub fn clients_toml(file: &ClientsFile) -> Result<String, RegistryError> {
    toml::to_string(file).map_err(|e| RegistryError::Mailo(e.to_string()))
}

#[derive(Deserialize)]
struct MailoFile {
    #[serde(default)]
    registrations: Vec<MailoRegistration>,
}

#[derive(Deserialize)]
struct MailoRegistration {
    issuer: Issuer,
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    endpoints: Option<MailoEndpoints>,
}

#[derive(Deserialize)]
struct MailoEndpoints {
    auth: String,
    token: String,
}

/// Reads mailo's `oauth.json` (the `OAuthRegistry` of `signin.rs`) as clients for `channel`
/// (mailo has no channels, so the caller says which build the ids were registered for). Mailo's
/// overridden `auth`/`token` become the entry's endpoints, with revoke and device absent: a
/// sovereign-cloud or proxy override is not known to have either. The result is for the person's
/// own file.
pub fn from_mailo(text: &str, channel: ClientChannel) -> Result<ClientsFile, RegistryError> {
    let mailo: MailoFile =
        serde_json::from_str(text).map_err(|e| RegistryError::Mailo(e.to_string()))?;
    let endpoint = |url: &str| {
        EndpointUrl::parse(url).map_err(|e| RegistryError::Mailo(format!("{url}: {e}")))
    };
    let mut clients: Vec<ClientEntry> = Vec::new();
    for r in mailo.registrations {
        let endpoints = r
            .endpoints
            .map(|e| {
                Ok::<_, RegistryError>(IssuerEndpoints {
                    authorize: endpoint(&e.auth)?,
                    token: endpoint(&e.token)?,
                    revoke: None,
                    device: None,
                })
            })
            .transpose()?;
        // Mailo's `set` replaces per issuer, so a file has at most one row each; a duplicate in a
        // hand-edited file keeps the last, as mailo's `get` would not, but the file is wrong anyway.
        clients.retain(|c| c.issuer != r.issuer);
        clients.push(ClientEntry {
            issuer: r.issuer,
            channel,
            client_id: ClientId(r.client_id),
            client_secret: r.client_secret.map(SecretText::new),
            endpoints,
        });
    }
    Ok(ClientsFile { clients })
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_provider::parse_clients;

    fn file(text: &str) -> ClientsFile {
        parse_clients(text).expect("parses")
    }

    #[test]
    fn the_persons_own_client_wins_and_others_fall_through() {
        let shipped = file(
            "[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"shipped-g\"\n\
             [[client]]\nissuer = \"microsoft\"\nchannel = \"stable\"\nclient_id = \"shipped-ms\"\n",
        );
        let own =
            file("[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"mine\"\n");
        let registry = ClientRegistry::layered(shipped, own);
        let id = |issuer, channel| {
            registry
                .lookup(issuer, channel)
                .map(|c| c.client_id.0.as_str())
        };
        assert_eq!(id(Issuer::Google, ClientChannel::Stable), Some("mine"));
        assert_eq!(
            id(Issuer::Microsoft, ClientChannel::Stable),
            Some("shipped-ms")
        );
        assert_eq!(id(Issuer::Microsoft, ClientChannel::Beta), None);
        assert_eq!(id(Issuer::Dropbox, ClientChannel::Stable), None);
    }

    const MAILO_FIXTURE: &str = include_str!("../tests/fixtures/mailo-oauth.json");

    #[test]
    fn mailos_registry_file_becomes_clients() {
        let file = from_mailo(MAILO_FIXTURE, ClientChannel::Stable).expect("reads");
        let rows: Vec<_> = file
            .clients
            .iter()
            .map(|c| {
                (
                    c.issuer,
                    c.channel,
                    c.client_id.0.as_str(),
                    c.client_secret.as_ref().map(SecretText::expose),
                    c.endpoints.as_ref().map(|e| e.token.as_str()),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    Issuer::Google,
                    ClientChannel::Stable,
                    "123.apps.googleusercontent.com",
                    Some("GOCSPX-example"),
                    None
                ),
                (
                    Issuer::Microsoft,
                    ClientChannel::Stable,
                    "ms-client",
                    None,
                    Some("https://login.microsoftonline.us/organizations/oauth2/v2.0/token")
                ),
            ]
        );
        // And it lands in a file the registry reads back identically.
        let text = clients_toml(&file).expect("toml");
        assert_eq!(parse_clients(&text), Ok(file));
    }

    #[test]
    fn a_damaged_mailo_file_is_an_error_not_an_empty_list() {
        assert!(matches!(
            from_mailo("{ not json", ClientChannel::Stable),
            Err(RegistryError::Mailo(_))
        ));
        assert_eq!(
            from_mailo("{}", ClientChannel::Stable),
            Ok(ClientsFile::default())
        );
    }

    #[test]
    fn paths_are_read_layered_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("porter-oauth-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let shipped = dir.join("shipped.toml");
        let own = dir.join("own.toml");
        std::fs::write(
            &shipped,
            "[[client]]\nissuer = \"microsoft\"\nchannel = \"stable\"\nclient_id = \"ours\"\n",
        )
        .expect("write");
        let ms = |r: &ClientRegistry| {
            r.lookup(Issuer::Microsoft, ClientChannel::Stable)
                .map(|c| c.client_id.0.clone())
        };
        let alone = ClientRegistry::from_paths(&shipped, &dir.join("absent.toml")).expect("reads");
        assert_eq!(ms(&alone), Some("ours".into()));
        std::fs::write(
            &own,
            "[[client]]\nissuer = \"microsoft\"\nchannel = \"stable\"\nclient_id = \"mine\"\n",
        )
        .expect("write");
        let both = ClientRegistry::from_paths(&shipped, &own).expect("reads");
        assert_eq!(ms(&both), Some("mine".into()));
        std::fs::write(&own, "[[client").expect("write");
        assert!(matches!(
            ClientRegistry::from_paths(&shipped, &own),
            Err(RegistryError::Invalid { .. })
        ));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn endpoints_fall_back_to_the_issuers_published_ones() {
        let registry = ClientRegistry::layered(
            file("[[client]]\nissuer = \"microsoft\"\nchannel = \"stable\"\nclient_id = \"x\"\n"),
            ClientsFile::default(),
        );
        let client = registry
            .lookup(Issuer::Microsoft, ClientChannel::Stable)
            .expect("client");
        assert_eq!(endpoints_of(client), Issuer::Microsoft.endpoints());
    }

    #[test]
    fn a_rows_byo_and_testing_are_read_and_the_persons_row_wins() {
        let dir = std::env::temp_dir().join(format!("porter-oauth-traits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (shipped, own) = (dir.join("shipped.toml"), dir.join("own.toml"));
        std::fs::write(
            &shipped,
            "[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"s\"\ntesting = true\n",
        )
        .expect("write");
        let at = |r: &ClientRegistry, c| r.traits(Issuer::Google, c);
        let alone = ClientRegistry::from_paths(&shipped, &dir.join("absent.toml")).expect("reads");
        assert_eq!(
            at(&alone, ClientChannel::Stable),
            ClientTraits {
                mail: MailRights::Withheld,
                review: AppReview::Testing
            }
        );
        std::fs::write(
            &own,
            "[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"m\"\nbyo = true\n",
        )
        .expect("write");
        let both = ClientRegistry::from_paths(&shipped, &own).expect("reads");
        // The row the lookup finds is the person's, so are its traits: not a mix of the two.
        assert_eq!(
            at(&both, ClientChannel::Stable),
            ClientTraits {
                mail: MailRights::Byo,
                review: AppReview::Verified
            }
        );
        // No row, no traits to speak of.
        assert_eq!(at(&both, ClientChannel::Beta), ClientTraits::default());
        let set = ClientRegistry::default().with_traits(
            Issuer::Google,
            ClientChannel::Stable,
            ClientTraits {
                mail: MailRights::Byo,
                review: AppReview::Testing,
            },
        );
        // A traits entry for a row that does not exist says nothing.
        assert_eq!(at(&set, ClientChannel::Stable), ClientTraits::default());
        let held = ClientRegistry::layered(
            file("[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"s\"\n"),
            ClientsFile::default(),
        )
        .with_traits(
            Issuer::Google,
            ClientChannel::Stable,
            ClientTraits {
                mail: MailRights::Byo,
                review: AppReview::Testing,
            },
        );
        assert_eq!(at(&held, ClientChannel::Stable).review, AppReview::Testing);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
