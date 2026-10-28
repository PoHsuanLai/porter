//! What a Graph token shows, asked: who the account is, and one small read per service (the
//! paths are Microsoft's `probe` rows of `issuers.toml`). What each answer means is
//! oauth-probe-proto's.

use crate::oauth::{failure, get};
use oauth_probe_proto::microsoft::{self, ME, is_personal};
use porter_capability::Claim;
use porter_core::EndpointUrl;
use porter_http::Http;
use porter_provider::{Issuer, ProviderError, ProviderSpec};

/// What probing an account found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    /// The account's address: its mail address, else its user principal name.
    pub address: String,
    /// What each declared kind came to.
    pub claims: Vec<Claim>,
}

impl Found {
    /// Whether a tenant refused any permission.
    pub fn tenant_refused(&self) -> bool {
        microsoft::tenant_refused(&self.claims)
    }
}

/// The account's address: its mail address, else its user principal name.
pub(super) async fn whoami<H: Http>(
    http: &H,
    base: &EndpointUrl,
    token: &str,
) -> Result<String, ProviderError> {
    let me = get(http, &at(base, ME), token).await?;
    microsoft::address(&me).map_err(failure)
}

/// Probes the account behind `token` for every kind `spec` declares.
pub(super) async fn probe<H: Http>(
    http: &H,
    spec: &ProviderSpec,
    base: &EndpointUrl,
    token: &str,
) -> Result<Found, ProviderError> {
    let address = whoami(http, base, token).await?;
    let personal = is_personal(&address);
    let mut claims = Vec::new();
    let mut seen = Vec::new();
    // An on-use row (the whole drive) is not the account's until it is asked for.
    for row in spec.sign_in_rows() {
        let kind = row.capability.kind();
        if seen.contains(&kind) {
            continue;
        }
        seen.push(kind);
        let read = Issuer::Microsoft.data().probe(kind);
        if let Some(claim) = microsoft::settled(&row.capability, read.is_some()) {
            claims.push(claim);
            continue;
        }
        let read = read.ok_or(ProviderError::Unreadable)?;
        let answer = get(http, &at(base, &read.path), token).await?;
        claims.push(microsoft::verdict(&row.capability, personal, &answer).map_err(failure)?);
    }
    Ok(Found { address, claims })
}

/// `path` under `base`.
fn at(base: &EndpointUrl, path: &str) -> String {
    format!("{}{path}", base.as_str().trim_end_matches('/'))
}
