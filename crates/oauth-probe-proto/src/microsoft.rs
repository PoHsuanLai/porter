//! What a Graph token shows: who the account is, and which of its services answer. One small
//! read per service; a 403 is the tenant refusing the permission (`Absent{TenantConsent}` at
//! `Probed`, R13), a 404 is a service the account has none of.
//!
//! Requests carry no query string: each path answers with its default page.

use crate::Failure;
use http::Response;
use porter_capability::{
    AbsentReason, Capability, CapabilityKind, Claim, Offer, Provenance, StorageCap, Subject,
};
use serde::Deserialize;

/// The path of Graph's `/me`, which says who the account is.
pub const ME: &str = "/v1.0/me";

/// The consumer domains of Microsoft accounts: no organisation, so no tenant to consent.
const PERSONAL_DOMAINS: [&str; 4] = ["outlook.com", "hotmail.com", "live.com", "msn.com"];

/// Whether `address` names a personal (consumer) Microsoft account rather than a work or school
/// account in an organisation's tenant. The rule is mailo's (`is_personal_microsoft` in its
/// `presets.rs`): the four consumer domains are personal, anything else is a tenant's.
pub fn is_personal(address: &str) -> bool {
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
    PERSONAL_DOMAINS.contains(&domain.trim().to_ascii_lowercase().as_str())
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

/// The account's address from the `answer` to the read of [`ME`]: its mail address, else its
/// user principal name.
pub fn address(answer: &Response<Vec<u8>>) -> Result<String, Failure> {
    let me: Me = match answer.status().as_u16() {
        200..=299 => serde_json::from_slice(answer.body()).map_err(|_| Failure::Unreadable)?,
        status => return Err(Failure::Status(status)),
    };
    [me.mail, me.user_principal_name]
        .into_iter()
        .flatten()
        .find(|a| !a.is_empty())
        .ok_or(Failure::Unreadable)
}

fn claim(offer: Offer, provenance: Provenance) -> Claim {
    Claim {
        subject: Subject::Account,
        offer,
        provenance,
    }
}

/// The claim for the `declared` capability that needs no read, when it has none: Mail is IMAP,
/// not Graph, so the token's grant is what shows it.
pub fn settled(declared: &Capability, has_read: bool) -> Option<Claim> {
    (!has_read).then(|| claim(Offer::Present(declared.clone()), Provenance::Discovered))
}

/// What the `answer` to the read of the `declared` capability's service says about the account.
/// Any status but a success, a 403 and a 404 is the caller's to report.
pub fn verdict(
    declared: &Capability,
    personal: bool,
    answer: &Response<Vec<u8>>,
) -> Result<Claim, Failure> {
    let kind: CapabilityKind = declared.kind();
    let absent = |reason| Offer::Absent { kind, reason };
    Ok(match answer.status().as_u16() {
        200..=299 => claim(
            Offer::Present(with_quota(declared, answer)),
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
        status => return Err(Failure::Status(status)),
    })
}

/// Whether a tenant refused any permission.
pub fn tenant_refused(claims: &[Claim]) -> bool {
    claims.iter().any(|c| {
        matches!(
            c.offer,
            Offer::Absent {
                reason: AbsentReason::TenantConsent,
                ..
            }
        )
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
            Capability::Storage(StorageCap {
                quota: reported,
                ..storage.clone()
            })
        }
        other => other.clone(),
    }
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

    fn answer(status: u16, body: &str) -> Response<Vec<u8>> {
        Response::builder()
            .status(status)
            .body(body.as_bytes().to_vec())
            .unwrap()
    }

    #[test]
    fn the_address_is_the_mail_else_the_user_principal_name() {
        let got = |body: &str| address(&answer(200, body)).ok();
        assert_eq!(
            got(r#"{"mail":"a@x.test","userPrincipalName":"u@x.test"}"#).as_deref(),
            Some("a@x.test")
        );
        assert_eq!(
            got(r#"{"mail":"","userPrincipalName":"u@x.test"}"#).as_deref(),
            Some("u@x.test")
        );
        assert_eq!(got(r#"{}"#), None);
        assert_eq!(address(&answer(401, "")).unwrap_err(), Failure::Status(401));
    }

    fn drive() -> Capability {
        use porter_capability::{Access, Delta, HashKind, StorageScope};
        Capability::Storage(StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: true,
            scope: StorageScope::AppFolder,
            hashes: HashKind::Sha1,
            ranges: true,
            chunked_upload: false,
        })
    }

    #[test]
    fn an_answer_is_present_refused_by_the_tenant_or_the_service_or_missing() {
        let claim = |personal, status, body: &str| {
            verdict(&drive(), personal, &answer(status, body))
                .unwrap()
                .offer
        };
        let Offer::Present(Capability::Storage(with)) =
            claim(false, 200, r#"{"quota":{"total":5}}"#)
        else {
            panic!("present")
        };
        assert!(with.quota);
        let Offer::Present(Capability::Storage(without)) = claim(false, 200, "{}") else {
            panic!("present")
        };
        assert!(!without.quota, "OneDrive reported none");
        assert!(matches!(
            claim(false, 403, ""),
            Offer::Absent {
                reason: AbsentReason::TenantConsent,
                ..
            }
        ));
        assert!(matches!(
            claim(true, 403, ""),
            Offer::Absent {
                reason: AbsentReason::ProviderOffersNone,
                ..
            }
        ));
        assert!(matches!(
            claim(true, 404, ""),
            Offer::Absent {
                reason: AbsentReason::NotOnServer,
                ..
            }
        ));
        assert_eq!(
            verdict(&drive(), true, &answer(500, "")).unwrap_err(),
            Failure::Status(500)
        );
    }

    #[test]
    fn a_kind_with_no_read_is_shown_by_the_grant_and_a_tenant_refusal_is_found() {
        assert!(settled(&drive(), true).is_none());
        let claim = settled(&drive(), false).unwrap();
        assert_eq!(claim.provenance, Provenance::Discovered);
        assert!(!tenant_refused(std::slice::from_ref(&claim)));
        let refused = verdict(&drive(), false, &answer(403, "")).unwrap();
        assert!(tenant_refused(&[claim, refused]));
    }
}
