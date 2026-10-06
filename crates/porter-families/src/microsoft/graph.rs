//! What a Graph token shows: who the account is, and which of its services answer. One small
//! read per service; a 403 is the tenant refusing the permission (`Absent{TenantConsent}` at
//! `Probed`, R13), a 404 is a service the account has none of.
//!
//! Requests carry no query string (an `EndpointUrl` has none): each path answers with its
//! default page.

use super::scopes::{AccountClass, classify};
use porter_core::capability::{Capability, CapabilityKind, QuotaReport};
use porter_core::{AbsentReason, Claim, EndpointUrl, Offer, Provenance, Subject};
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_provider::{ProviderError, ProviderSpec};
use serde::Deserialize;

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
    let me = get(http, base, "/v1.0/me", token).await?;
    let me: Me = match me.status.0 {
        200..=299 => serde_json::from_slice(&me.body).map_err(|_| ProviderError::Unreadable)?,
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
    let class = classify(&address);
    let mut claims = Vec::new();
    let mut seen = Vec::new();
    for row in &spec.capabilities {
        let kind = row.capability.kind();
        if seen.contains(&kind) {
            continue;
        }
        seen.push(kind);
        claims.push(claim_for(http, base, token, class, &row.capability).await?);
    }
    Ok(Found { address, claims })
}

async fn claim_for<H: Http>(
    http: &H,
    base: &EndpointUrl,
    token: &str,
    class: AccountClass,
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
    let answer = get(http, base, path, token).await?;
    let absent = |reason| Offer::Absent { kind, reason };
    Ok(match answer.status.0 {
        200..=299 => claim(
            Offer::Present(with_quota(declared, &answer)),
            Provenance::Probed,
        ),
        // A personal account has no tenant to refuse: a 403 there is the service itself.
        403 => claim(
            absent(match class {
                AccountClass::Work => AbsentReason::TenantConsent,
                AccountClass::Personal => AbsentReason::ProviderOffersNone,
            }),
            Provenance::Probed,
        ),
        404 => claim(absent(AbsentReason::NotOnServer), Provenance::Probed),
        status => return Err(fault(status)),
    })
}

/// OneDrive says whether it reports a quota; the declared row assumed it does.
fn with_quota(declared: &Capability, answer: &HttpResponse) -> Capability {
    match declared {
        Capability::Storage(storage) => {
            let reported = serde_json::from_slice::<Drive>(&answer.body)
                .ok()
                .and_then(|d| d.quota)
                .and_then(|q| q.total)
                .is_some();
            Capability::Storage(porter_core::capability::StorageCap {
                quota: match reported {
                    true => QuotaReport::Reported,
                    false => QuotaReport::Unreported,
                },
                ..storage.clone()
            })
        }
        other => other.clone(),
    }
}

/// What a non-success status means for the account.
fn fault(status: u16) -> ProviderError {
    match status {
        401 => ProviderError::Unauthorized,
        403 => ProviderError::Forbidden,
        429 | 500..=599 => ProviderError::Unreachable,
        _ => ProviderError::Unreadable,
    }
}

async fn get<H: Http>(
    http: &H,
    base: &EndpointUrl,
    path: &str,
    token: &str,
) -> Result<HttpResponse, ProviderError> {
    let url = EndpointUrl::parse(&format!("{}{path}", base.as_str().trim_end_matches('/')))
        .map_err(|_| ProviderError::Unreadable)?;
    let request = HttpRequest::to(Method::Get, &url)
        .map_err(|_| ProviderError::Unreadable)?
        .with_header("Authorization", format!("Bearer {token}"))
        .with_header("Accept", "application/json");
    http.send(request)
        .await
        .map_err(|_| ProviderError::Unreachable)
}
