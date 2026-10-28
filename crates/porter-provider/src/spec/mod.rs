//! A provider declaration: the parsed form of `providers/<id>.toml` (design/31 §3.1). The
//! serde form is the file format.

mod auth;
mod discovery;
mod issuer;
mod linked;
mod matching;

pub use auth::{AuthSpec, Issuer, IssuerEndpoints};
pub use discovery::{Discovery, Port};
pub use issuer::{ApiRow, IssuerData, ScopeClass, ScopeRow};
pub use linked::{AuthOrigin, LinkedOrigin};
pub use matching::{DomainMatch, DomainName, Matching};

use porter_capability::{Ask, StorageScope};
use porter_capability::{Billing, Locality};
use porter_capability::{Capability, CapabilityKind};
use porter_core::AuthKind;
use porter_core::Family;
use porter_core::ProviderId;
use porter_sheet::{MarkFace, ProviderGroup, ProviderKind, ProviderRow, RowKind};
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
/// `kind` and fields sit beside `family`:
///
/// ```toml
/// [[capability]]
/// family = "webdav"
/// kind = "storage"
/// access = "read_write"
/// delta = "poll"
/// quota = "reported"
/// scope = "full"
/// hashes = "none"
/// ranges = "present"
/// chunked_upload = "absent"
/// ```
///
/// The types are the bus form (`kind` and a `v` table of the fields); `parse_provider` moves the
/// fields under `v`, and reads a file that still writes `v` as it is.
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
    /// When the person is asked for this row's access: with the sign-in (the default, not
    /// written), or `ask = "on_use"`, when something first uses it, in a sign-in that asks for
    /// it alone (`SignInMode::Extend`). A sign-in leaves an on-use row's access out, and the
    /// account's claims leave the row out until then.
    #[serde(default, skip_serializing_if = "Ask::is_sign_in")]
    pub ask: Ask,
}

impl CapabilityRow {
    /// Whether this row's access is asked with the sign-in.
    pub fn asked_at_sign_in(&self) -> bool {
        self.ask == Ask::SignIn
    }

    /// Whether this row reaches as far as a grant of `reach` goes: a Storage row as far as its
    /// scope, every other row as far as any grant (only Storage has a reach of its own).
    pub fn reaches(&self, reach: StorageScope) -> bool {
        match &self.capability {
            Capability::Storage(storage) => storage.scope >= reach,
            _ => true,
        }
    }
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

    /// The rows whose access a sign-in asks for, in file order: every row but the on-use ones.
    pub fn sign_in_rows(&self) -> impl Iterator<Item = &CapabilityRow> {
        self.capabilities
            .iter()
            .filter(|row| row.asked_at_sign_in())
    }

    /// The kinds a sign-in asks for, once each, in file order.
    pub fn sign_in_kinds(&self) -> Vec<CapabilityKind> {
        self.sign_in_rows()
            .map(|row| row.capability.kind())
            .fold(Vec::new(), |mut kinds, kind| {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
                kinds
            })
    }

    /// The rows of `kind` whose access is asked when first used.
    pub fn on_use_rows(&self, kind: CapabilityKind) -> impl Iterator<Item = &CapabilityRow> {
        self.capabilities
            .iter()
            .filter(move |row| row.ask == Ask::OnUse && row.capability.kind() == kind)
    }

    /// When the access a token for a grant of `kind` and `reach` carries is asked, from the rows
    /// of the kind that reach that far ([`CapabilityRow::reaches`]): with the sign-in when one of
    /// them is a sign-in row (the app folder, for a grant of the app folder), else on use (the
    /// whole drive, for a grant of the whole store); `None` when no row of the file reaches.
    pub fn ask_for(&self, kind: CapabilityKind, reach: StorageScope) -> Option<Ask> {
        let mut rows = self
            .capabilities
            .iter()
            .filter(|row| row.capability.kind() == kind && row.reaches(reach));
        match rows.clone().any(CapabilityRow::asked_at_sign_in) {
            true => Some(Ask::SignIn),
            false => rows.next().map(|_| Ask::OnUse),
        }
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
