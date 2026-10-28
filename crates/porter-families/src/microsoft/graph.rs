//! What a Graph token shows: who the account is, and which of its services answer. One small
//! read per service; a 403 is the tenant refusing the permission (`Absent{TenantConsent}` at
//! `Probed`, R13), a 404 is a service the account has none of.
//!
//! Requests carry no query string: each path answers with its default page.

use crate::oauth::{fault, get};
use porter_capability::{AbsentReason, Claim, Offer, Provenance, Subject};
use porter_capability::{Capability, CapabilityKind};
use porter_core::EndpointUrl;
use porter_http::Http;
use porter_http::http::Response;
use porter_provider::{ProviderError, ProviderSpec};
use serde::Deserialize;

/// The consumer domains of Microsoft accounts: no organisation, so no tenant to consent.
const PERSONAL_DOMAINS: [&str; 4] = ["outlook.com", "hotmail.com", "live.com", "msn.com"];

/// Whether `address` names a personal (consumer) Microsoft account rather than a work or school
/// account in an organisation's tenant. The rule is mailo's (`is_personal_microsoft` in its
/// `presets.rs`): the four consumer domains are personal, anything else is a tenant's.
pub fn is_personal(address: &str) -> bool {
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
    PERSONAL_DOMAINS.contains(&domain.trim().to_ascii_lowercase().as_str())
}

/// The Graph path whose answer shows a service is reachable.
fn probe_path(kind: CapabilityKind) -> Option<&'static str> {
    match kind {
        CapabilityKind::Calendar => Some("/v1.0/me/calendars"),
        CapabilityKind::Contacts => Some("/v1.0/me/contacts"),
        CapabilityKind::Tasks => Some("/v1.0/me/todo/lists"),
        CapabilityKind::Notes => Some("/v1.0/me/onenote/notebooks"),
        CapabilityKind::Storage => Some("/v1.0/me/drive"),
        _ => None,
    }
}

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
        self.claims.iter().any(|c| {
            matches!(
                c.offer,
                Offer::Absent {
                    reason: AbsentReason::TenantConsent,
                    ..
                }
            )
        })
    }
}

#[derive(Deserialize)]
struct Me {
    mail: Option<String>,
    #[serde(rename = "userPrincipalName")]
    user_principal_name: Option<String>,
}

#[derive(Deserialize)]
struct Drive {
    quota: Option<Quota>,
}

#[derive(Deserialize)]
struct Quota {
    total: Option<u64>,
}

/// The account's address: its mail address, else its user principal name.
pub(super) async fn whoami<H: Http>(
    http: &H,
    base: &EndpointUrl,
    token: &str,
) -> Result<String, ProviderError> {
    let me = get(http, &at(base, "/v1.0/me"), token).await?;
    let me: Me = match me.status().as_u16() {
        200..=299 => serde_json::from_slice(me.body()).map_err(|_| ProviderError::Unreadable)?,
        status => return Err(fault(status)),
    };
    [me.mail, me.user_principal_name]
        .into_iter()
        .flatten()
        .find(|a| !a.is_empty())
        .ok_or(ProviderError::Unreadable)
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
        claims.push(claim_for(http, base, token, personal, &row.capability).await?);
    }
    Ok(Found { address, claims })
}

async fn claim_for<H: Http>(
    http: &H,
    base: &EndpointUrl,
    token: &str,
    personal: bool,
    declared: &Capability,
) -> Result<Claim, ProviderError> {
    let kind = declared.kind();
    let claim = |offer, provenance| Claim {
        subject: Subject::Account,
        offer,
        provenance,
    };
    let Some(path) = probe_path(kind) else {
        // Mail is IMAP, not Graph: the token's grant is what shows it.
        return Ok(claim(
            Offer::Present(declared.clone()),
            Provenance::Discovered,
        ));
    };
    let answer = get(http, &at(base, path), token).await?;
    let absent = |reason| Offer::Absent { kind, reason };
    Ok(match answer.status().as_u16() {
        200..=299 => claim(
            Offer::Present(with_quota(declared, &answer)),
            Provenance::Probed,
        ),
        // A personal account has no tenant to refuse: a 403 there is the service itself.
        403 => claim(
            absent(match personal {
                false => AbsentReason::TenantConsent,
                true => AbsentReason::ProviderOffersNone,
            }),
            Provenance::Probed,
        ),
        404 => claim(absent(AbsentReason::NotOnServer), Provenance::Probed),
        status => return Err(fault(status)),
    })
}

/// OneDrive says whether it reports a quota; the declared row assumed it does.
fn with_quota(declared: &Capability, answer: &Response<Vec<u8>>) -> Capability {
    match declared {
        Capability::Storage(storage) => {
            let reported = serde_json::from_slice::<Drive>(answer.body())
                .ok()
                .and_then(|d| d.quota)
                .and_then(|q| q.total)
                .is_some();
            Capability::Storage(porter_capability::StorageCap {
                quota: reported,
                ..storage.clone()
            })
        }
        other => other.clone(),
    }
}

/// `path` under `base`.
fn at(base: &EndpointUrl, path: &str) -> String {
    format!("{}{path}", base.as_str().trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_and_work_addresses_are_told_apart_by_domain() {
        const CASES: &[(&str, bool)] = &[
            ("me@outlook.com", true),
            ("me@Hotmail.com", true),
            ("me@live.com", true),
            ("me@msn.com", true),
            ("me@contoso.onmicrosoft.com", false),
            ("me@firm.example", false),
            ("me@notoutlook.com", false),
            ("not an address", false),
        ];
        for (address, want) in CASES {
            assert_eq!(is_personal(address), *want, "{address}");
        }
    }
}
