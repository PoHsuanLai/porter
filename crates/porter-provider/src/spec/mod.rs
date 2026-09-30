//! A provider declaration: the parsed form of `providers/<id>.toml` (design/31 §3.1). The
//! serde form is the file format.

mod auth;
mod discovery;

pub use auth::{AuthSpec, Issuer};
pub use discovery::{Discovery, Port};

use crate::family::Family;
use porter_core::{Billing, Capability, Locality, ProviderId};
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
    /// How its accounts sign in.
    pub auth: AuthSpec,
    /// How an account's servers and capabilities are found.
    pub discovery: Discovery,
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
