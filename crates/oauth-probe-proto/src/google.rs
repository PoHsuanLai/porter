//! What a Google token shows: which of the account's services answer. One small read per service
//! that has one; a service whose scope the person unticked is absent without asking, and Photos,
//! which has no read that is not the person's whole library, is shown by the scope alone.
//!
//! A 403 is one of three things, told apart by what Google writes in the error: the API is not
//! switched on in the owner's Cloud project (`UnverifiedBuild`: the build cannot use it, and the
//! owner can fix it), a Workspace administrator turned the service off (`TenantConsent`), or a
//! personal account that has no such service (`ProviderOffersNone`).

use crate::Failure;
use http::Response;
use porter_capability::{
    AbsentReason, Capability, CapabilityKind, Claim, Offer, Provenance, Subject,
};

/// Whether an address is a consumer's (no administrator to blame for a refusal).
fn is_personal(address: &str) -> bool {
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
    matches!(
        domain.trim().to_ascii_lowercase().as_str(),
        "gmail.com" | "googlemail.com"
    )
}

fn claim(offer: Offer, provenance: Provenance) -> Claim {
    Claim {
        subject: Subject::Account,
        offer,
        provenance,
    }
}

/// The claim for `kind` that needs no read, when there is one: Mail without the restricted
/// scope (this build cannot read mail), a service the person unticked (`covers` is false), and
/// a kind with no cheap read (`has_read` is false), which the grant alone shows as `granted`.
pub fn settled(
    kind: CapabilityKind,
    (byo, covers, has_read): (bool, bool, bool),
    granted: &Capability,
) -> Option<Claim> {
    let absent = |reason| Offer::Absent { kind, reason };
    if kind == CapabilityKind::Mail && !byo {
        // The restricted scope was never asked: this build cannot read mail (R2).
        return Some(claim(
            absent(AbsentReason::UnverifiedBuild),
            Provenance::Declared,
        ));
    }
    if !covers {
        return Some(claim(
            absent(AbsentReason::TurnedOff),
            Provenance::Discovered,
        ));
    }
    // Mail is IMAP and Photos has no cheap read: the token's grant is what shows them.
    (!has_read).then(|| claim(Offer::Present(granted.clone()), Provenance::Discovered))
}

/// What the `answer` to the read of `kind`'s service says about the account at `address`. Any
/// status but a success, a 403 and a 404 is the caller's to report.
pub fn verdict(
    kind: CapabilityKind,
    granted: &Capability,
    address: &str,
    answer: &Response<Vec<u8>>,
) -> Result<Claim, Failure> {
    let absent = |reason| Offer::Absent { kind, reason };
    Ok(match answer.status().as_u16() {
        200..=299 => claim(Offer::Present(granted.clone()), Provenance::Probed),
        403 => claim(absent(refusal_reason(answer, address)), Provenance::Probed),
        404 => claim(absent(AbsentReason::NotOnServer), Provenance::Probed),
        status => return Err(Failure::Status(status)),
    })
}

/// Why Google said 403, from what it wrote.
fn refusal_reason(answer: &Response<Vec<u8>>, address: &str) -> AbsentReason {
    let body = String::from_utf8_lossy(answer.body());
    if body.contains("accessNotConfigured") || body.contains("SERVICE_DISABLED") {
        return AbsentReason::UnverifiedBuild;
    }
    match is_personal(address) {
        true => AbsentReason::ProviderOffersNone,
        false => AbsentReason::TenantConsent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_403_is_told_apart_by_what_google_wrote_and_who_asked() {
        let body = |text: &str| -> Response<Vec<u8>> {
            http::Response::builder()
                .status(403)
                .body(text.as_bytes().to_vec())
                .expect("response")
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

    fn answer(status: u16) -> Response<Vec<u8>> {
        Response::builder().status(status).body(Vec::new()).unwrap()
    }

    fn calendar() -> Capability {
        use porter_capability::{Access, Delta, PimCap, PimTransport};
        Capability::Calendar(PimCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            transport: PimTransport::CalDav,
            collections: true,
        })
    }

    #[test]
    fn an_answer_is_present_refused_or_missing_and_another_status_is_the_callers() {
        let claim = |status| {
            verdict(
                CapabilityKind::Calendar,
                &calendar(),
                "ada@gmail.com",
                &answer(status),
            )
        };
        assert!(matches!(claim(200).unwrap().offer, Offer::Present(_)));
        assert_eq!(claim(200).unwrap().provenance, Provenance::Probed);
        assert!(matches!(
            claim(403).unwrap().offer,
            Offer::Absent {
                reason: AbsentReason::ProviderOffersNone,
                ..
            }
        ));
        assert!(matches!(
            claim(404).unwrap().offer,
            Offer::Absent {
                reason: AbsentReason::NotOnServer,
                ..
            }
        ));
        assert_eq!(claim(500).unwrap_err(), Failure::Status(500));
    }

    #[test]
    fn what_needs_no_read_is_settled_without_one() {
        let cap = calendar();
        let offer = |c: Option<Claim>| c.map(|c| c.offer);
        // Mail needs the person's own client.
        assert!(matches!(
            offer(settled(CapabilityKind::Mail, (false, true, false), &cap)),
            Some(Offer::Absent {
                reason: AbsentReason::UnverifiedBuild,
                ..
            })
        ));
        // A service the person unticked.
        assert!(matches!(
            offer(settled(
                CapabilityKind::Calendar,
                (false, false, true),
                &cap
            )),
            Some(Offer::Absent {
                reason: AbsentReason::TurnedOff,
                ..
            })
        ));
        // A kind with no cheap read is shown by the grant.
        assert!(matches!(
            offer(settled(CapabilityKind::Photos, (false, true, false), &cap)),
            Some(Offer::Present(_))
        ));
        // A kind with a read is asked.
        assert!(settled(CapabilityKind::Calendar, (false, true, true), &cap).is_none());
    }
}
