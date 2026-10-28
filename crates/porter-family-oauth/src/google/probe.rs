//! What a Google token shows, asked: one small read per service that has one (the paths are
//! Google's `probe` rows of `issuers.toml`). What each answer means, and which services need no
//! read, is oauth-probe-proto's.

use super::apis_of;
use super::scopes::Granted;
use crate::oauth::{failure, get};
use oauth_probe_proto::google;
use porter_capability::{CapabilityKind, Claim};
use porter_http::Http;
use porter_provider::{Issuer, ProviderError, ProviderSpec};

/// Probes the account behind `token` for every kind `spec` declares.
pub(super) async fn probe<H: Http>(
    http: &H,
    spec: &ProviderSpec,
    address: &str,
    token: &str,
    (granted, byo): (&Granted, bool),
) -> Result<Vec<Claim>, ProviderError> {
    let mut claims = Vec::new();
    // An on-use row (the whole of Drive) is not the account's until it is asked for.
    for kind in spec.sign_in_kinds() {
        if spec.sign_in_rows().all(|row| row.capability.kind() != kind) {
            continue;
        }
        claims.push(claim_for(http, (spec, address, token), (granted, byo), kind).await?);
    }
    Ok(claims)
}

async fn claim_for<H: Http>(
    http: &H,
    (spec, address, token): (&ProviderSpec, &str, &str),
    (granted, byo): (&Granted, bool),
    kind: CapabilityKind,
) -> Result<Claim, ProviderError> {
    let declared = spec
        .sign_in_rows()
        .find(|row| row.capability.kind() == kind)
        .map(|row| granted.narrow(&row.capability))
        .ok_or(ProviderError::Unreadable)?;
    let row = Issuer::Google.data().probe(kind);
    if let Some(claim) =
        google::settled(kind, (byo, granted.covers(kind), row.is_some()), &declared)
    {
        return Ok(claim);
    }
    let row = row.ok_or(ProviderError::Unreadable)?;
    let base = apis_of(spec).base(row.family);
    let answer = get(
        http,
        &format!("{}{}", base.as_str().trim_end_matches('/'), row.path),
        token,
    )
    .await?;
    google::verdict(kind, &declared, address, &answer).map_err(failure)
}
