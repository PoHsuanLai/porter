//! What a Google token shows: who the account is, and which of its services answer. One small
//! read per service that has one; a service whose scope the person unticked is absent without
//! asking, and Photos, which has no read that is not the person's whole library, is shown by the
//! scope alone.
//!
//! A 403 is one of three things, told apart by what Google writes in the error: the API is not
//! switched on in the owner's Cloud project (`UnverifiedBuild`: the build cannot use it, and the
//! owner can fix it), a Workspace administrator turned the service off (`TenantConsent`), or a
//! personal account that has no such service (`ProviderOffersNone`).

use super::scopes::Granted;
use super::{apis_of, declared_kinds};
use porter_core::capability::{Capability, CapabilityKind};
use porter_core::{AbsentReason, Claim, EndpointUrl, Family, Offer, Provenance, Subject, WebUrl};
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_oauth::MailRights;
use porter_provider::{ProviderError, ProviderSpec};
use serde::Deserialize;

#[derive(Deserialize)]
struct UserInfo {
    email: Option<String>,
}

/// The account's address, from the userinfo endpoint.
pub(super) async fn whoami<H: Http>(
    http: &H,
    userinfo: &EndpointUrl,
    token: &str,
) -> Result<String, ProviderError> {
    let answer = get(http, userinfo.as_str(), token).await?;
    let info: UserInfo = match answer.status.0 {
        200..=299 => serde_json::from_slice(&answer.body).map_err(|_| ProviderError::Unreadable)?,
        status => return Err(fault(status)),
    };
    info.email
        .filter(|a| !a.is_empty())
        .ok_or(ProviderError::Unreadable)
}

/// Whether an address is a consumer's (no administrator to blame for a refusal).
fn is_personal(address: &str) -> bool {
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
    matches!(
        domain.trim().to_ascii_lowercase().as_str(),
        "gmail.com" | "googlemail.com"
    )
}

/// The path (with its query) whose answer shows a service is reachable.
fn probe_path(kind: CapabilityKind) -> Option<(Family, &'static str)> {
    match kind {
        CapabilityKind::Calendar => Some((
            Family::GoogleCalendar,
            "/users/me/calendarList?maxResults=1",
        )),
        CapabilityKind::Contacts => Some((Family::GooglePeople, "/contactGroups?pageSize=1")),
        CapabilityKind::Tasks => Some((Family::GoogleTasks, "/users/@me/lists?maxResults=1")),
        CapabilityKind::Storage => Some((
            Family::GoogleDrive,
            "/files?spaces=appDataFolder&pageSize=1&fields=files(id)",
        )),
        _ => None,
    }
}

/// Probes the account behind `token` for every kind `spec` declares.
pub(super) async fn probe<H: Http>(
    http: &H,
    spec: &ProviderSpec,
    address: &str,
    token: &str,
    (granted, mail): (&Granted, MailRights),
) -> Result<Vec<Claim>, ProviderError> {
    let mut claims = Vec::new();
    for kind in declared_kinds(spec) {
        let Some(declared) = spec
            .capabilities
            .iter()
            .find(|row| row.capability.kind() == kind)
            .map(|row| &row.capability)
        else {
            continue;
        };
        claims.push(
            claim_for(
                http,
                (spec, address, token),
                (granted, mail),
                kind,
                declared,
            )
            .await?,
        );
    }
    Ok(claims)
}

async fn claim_for<H: Http>(
    http: &H,
    (spec, address, token): (&ProviderSpec, &str, &str),
    (granted, mail): (&Granted, MailRights),
    kind: CapabilityKind,
    declared: &Capability,
) -> Result<Claim, ProviderError> {
    let claim = |offer, provenance| Claim {
        subject: Subject::Account,
        offer,
        provenance,
    };
    let absent = |reason| Offer::Absent { kind, reason };
    let present = || Offer::Present(declared.clone());
    if kind == CapabilityKind::Mail && mail != MailRights::Byo {
        // The restricted scope was never asked: this build cannot read mail (R2).
        return Ok(claim(
            absent(AbsentReason::UnverifiedBuild),
            Provenance::Declared,
        ));
    }
    if !granted.covers(kind) {
        return Ok(claim(
            absent(AbsentReason::TurnedOff),
            Provenance::Discovered,
        ));
    }
    let Some((family, path)) = probe_path(kind) else {
        // Mail is IMAP and Photos has no cheap read: the token's grant is what shows them.
        return Ok(claim(present(), Provenance::Discovered));
    };
    let base = apis_of(spec).base(family);
    let answer = get(
        http,
        &format!("{}{path}", base.as_str().trim_end_matches('/')),
        token,
    )
    .await?;
    Ok(match answer.status.0 {
        200..=299 => claim(present(), Provenance::Probed),
        403 => claim(absent(refusal_reason(&answer, address)), Provenance::Probed),
        404 => claim(absent(AbsentReason::NotOnServer), Provenance::Probed),
        status => return Err(fault(status)),
    })
}

/// Why Google said 403, from what it wrote.
fn refusal_reason(answer: &HttpResponse, address: &str) -> AbsentReason {
    let body = String::from_utf8_lossy(&answer.body);
    if body.contains("accessNotConfigured") || body.contains("SERVICE_DISABLED") {
        return AbsentReason::UnverifiedBuild;
    }
    match is_personal(address) {
        true => AbsentReason::ProviderOffersNone,
        false => AbsentReason::TenantConsent,
    }
}

/// What a non-success status means for the account.
pub(super) fn fault(status: u16) -> ProviderError {
    match status {
        401 => ProviderError::Unauthorized,
        403 => ProviderError::Forbidden,
        429 | 500..=599 => ProviderError::Unreachable,
        _ => ProviderError::Unreadable,
    }
}

async fn get<H: Http>(http: &H, url: &str, token: &str) -> Result<HttpResponse, ProviderError> {
    let url = WebUrl::parse(url).map_err(|_| ProviderError::Unreadable)?;
    let request = HttpRequest::new(Method::Get, url)
        .with_header("Authorization", format!("Bearer {token}"))
        .with_header("Accept", "application/json");
    http.send(request)
        .await
        .map_err(|_| ProviderError::Unreachable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_403_is_told_apart_by_what_google_wrote_and_who_asked() {
        let body = |text: &str| HttpResponse {
            status: porter_http::Status(403),
            headers: Vec::new(),
            body: text.as_bytes().to_vec(),
        };
        const CASES: &[(&str, &str, &str, AbsentReason)] = &[
            (
                "api off",
                r#"{"error":{"errors":[{"reason":"accessNotConfigured"}]}}"#,
                "ada@gmail.com",
                AbsentReason::UnverifiedBuild,
            ),
            (
                "api off (v2 error)",
                r#"{"error":{"details":[{"reason":"SERVICE_DISABLED"}]}}"#,
                "ada@firm.example",
                AbsentReason::UnverifiedBuild,
            ),
            (
                "workspace admin",
                r#"{"error":{"message":"Forbidden"}}"#,
                "ada@firm.example",
                AbsentReason::TenantConsent,
            ),
            (
                "personal account",
                r#"{"error":{"message":"Forbidden"}}"#,
                "ada@GMail.com",
                AbsentReason::ProviderOffersNone,
            ),
            (
                "googlemail",
                "{}",
                "ada@googlemail.com",
                AbsentReason::ProviderOffersNone,
            ),
        ];
        for (name, text, address, want) in CASES {
            assert_eq!(refusal_reason(&body(text), address), *want, "{name}");
        }
    }

    #[test]
    fn statuses_map_to_what_the_account_needs() {
        const CASES: &[(u16, ProviderError)] = &[
            (401, ProviderError::Unauthorized),
            (403, ProviderError::Forbidden),
            (429, ProviderError::Unreachable),
            (503, ProviderError::Unreachable),
            (418, ProviderError::Unreadable),
        ];
        for (status, want) in CASES {
            assert_eq!(&fault(*status), want, "{status}");
        }
    }
}
