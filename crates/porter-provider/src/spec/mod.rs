//! A provider declaration: the parsed form of `providers/<id>.toml` (design/31 §3.1). The
//! serde form is the file format.

mod auth;
mod discovery;
mod linked;
mod matching;

pub use auth::{AuthSpec, Issuer, IssuerEndpoints};
pub use discovery::{Discovery, Port};
pub use linked::{AuthOrigin, LinkedOrigin};
pub use matching::{DomainMatch, DomainName, Matching};

use porter_core::AuthKind;
use porter_core::sheet::{MarkFace, ProviderGroup, ProviderKind, ProviderRow, RowKind};
use porter_core::{Billing, Capability, Family, Locality, ProviderId};
use serde::{Deserialize, Serialize};

/// One provider, as its file declares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSpec {
    /// Its id; the file name without `.toml`.
    pub id: ProviderId,
    /// What the user reads ("Nextcloud").
    pub label: String,
    /// The provider mark glyph's name (design/30 §2.11).
    pub mark: String,
    /// The letter and colour of its mark, for the UI to draw when it has no named mark for the
    /// provider. In the file this is the table `[mark_face]` (`letter`, `colour`; both
    /// required): `[mark]` cannot be a table beside the word `mark = "..."`, which is a TOML
    /// string key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_face: Option<MarkFace>,
    /// The group of the Accounts page it is listed under: `group = "internet"`, `"intelligence"`
    /// or `"agent"`, top level in the file. Every shipped file names one; a file that does not
    /// is placed by [`ProviderGroup::fallback`] (read it through [`ProviderSpec::group`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<ProviderGroup>,
    /// How its accounts sign in.
    pub auth: AuthSpec,
    /// How an account's servers and capabilities are found.
    pub discovery: Discovery,
    /// The address domains and MX hosts that mark an address as this provider's; empty for a
    /// provider no address implies (Nextcloud, a local runtime).
    #[serde(default)]
    pub matching: Matching,
    /// Where its AI models run and what they cost; absent for providers without AI kinds.
    #[serde(default)]
    pub ai: Option<AiSpec>,
    /// What it declares it can do, one row per capability.
    #[serde(rename = "capability")]
    pub capabilities: Vec<CapabilityRow>,
}

/// One declared capability and the family that serves it. In the file the capability's own
/// `kind` and `v` keys sit beside `family`:
///
/// ```toml
/// [[capability]]
/// family = "webdav"
/// kind = "storage"
/// v = { access = "read_write", delta = "poll", quota = "reported", scope = "full", hashes = "none", ranges = "present", chunked_upload = "absent" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRow {
    /// The protocol engine.
    pub family: Family,
    /// A fixed endpoint, when discovery does not find one.
    #[serde(default)]
    pub endpoint: Option<Endpoint>,
    /// The capability.
    #[serde(flatten)]
    pub capability: Capability,
    /// The other origins this row's service hands out pre-authenticated links to (Graph's
    /// `uploadUrl` and `downloadUrl`), which `OpenLinked` may reach for a grant of this row's
    /// kind with no credential added. Empty for a service with no such links.
    #[serde(default)]
    pub linked_origins: Vec<LinkedOrigin>,
    /// The other origins this row's service hands out links to that want the account's bearer
    /// (Google Photos' `baseUrl` on `lh3.googleusercontent.com`): `OpenAuthenticated` accepts
    /// each as an endpoint of the grant's kind and relays to it WITH the bearer of this row's
    /// family. Exact hosts, no wildcard, same scheme as the row's endpoint.
    #[serde(default)]
    pub auth_origins: Vec<AuthOrigin>,
}

/// A service URL written in a provider file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Endpoint(pub String);

/// The AI properties of a provider's models (design/31 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiSpec {
    /// Where they run.
    pub locality: Locality,
    /// How they are paid for.
    pub billing: Billing,
}

impl ProviderSpec {
    /// The group it is listed under: the file's, else the one decided from its sign-in and
    /// services.
    pub fn group(&self) -> ProviderGroup {
        self.group.unwrap_or_else(|| {
            let kinds: Vec<_> = self
                .capabilities
                .iter()
                .map(|r| r.capability.kind())
                .collect();
            ProviderGroup::fallback(self.auth.kind, &kinds)
        })
    }

    /// The row the sheet's provider list shows for this provider.
    pub fn sheet_row(&self) -> ProviderRow {
        ProviderRow {
            id: self.id.clone(),
            label: self.label.clone(),
            mark: self.mark.clone(),
            kind: match self.id.as_str().starts_with("generic-") {
                true => RowKind::Generic,
                false => RowKind::Provider,
            },
            auth: match self.auth.kind {
                AuthKind::AgentLogin => ProviderKind::AgentLogin,
                _ => ProviderKind::Service,
            },
            mark_face: self.mark_face.clone(),
            group: Some(self.group()),
        }
    }
}
