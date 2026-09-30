use super::*;
use crate::capability::{Access, Delta, StorageScope};
use crate::fixtures::{claim, storage};
use crate::offer::{Offer, Provenance, Subject};

fn poll() -> crate::Capability {
    storage(Access::ReadWrite, Delta::Poll, StorageScope::Full)
}
fn push() -> crate::Capability {
    storage(Access::ReadWrite, Delta::Push, StorageScope::Full)
}
fn absent(reason: AbsentReason) -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Absent {
            kind: CapabilityKind::Storage,
            reason,
        },
        provenance: Provenance::Probed,
    }
}

struct Case {
    name: &'static str,
    claims: Vec<Claim>,
    toggles: Vec<KindToggle>,
    expected: Vec<Claim>,
}

fn cases() -> Vec<Case> {
    let off = KindToggle {
        kind: CapabilityKind::Storage,
        toggle: Toggle::Off,
    };
    vec![
        Case {
            name: "declared alone holds",
            claims: vec![claim(poll(), Provenance::Declared)],
            toggles: vec![],
            expected: vec![claim(poll(), Provenance::Declared)],
        },
        Case {
            name: "discovered refines declared",
            claims: vec![
                claim(poll(), Provenance::Declared),
                claim(push(), Provenance::Discovered),
            ],
            toggles: vec![],
            expected: vec![claim(push(), Provenance::Discovered)],
        },
        Case {
            name: "declared after discovered does not override it",
            claims: vec![
                claim(push(), Provenance::Discovered),
                claim(poll(), Provenance::Declared),
            ],
            toggles: vec![],
            expected: vec![claim(push(), Provenance::Discovered)],
        },
        Case {
            name: "a probe corrects everything",
            claims: vec![
                claim(push(), Provenance::Discovered),
                absent(AbsentReason::TenantConsent),
            ],
            toggles: vec![],
            expected: vec![absent(AbsentReason::TenantConsent)],
        },
        Case {
            name: "a toggle turns the winner off",
            claims: vec![claim(poll(), Provenance::Declared)],
            toggles: vec![off],
            expected: vec![Claim {
                subject: Subject::Account,
                offer: Offer::Absent {
                    kind: CapabilityKind::Storage,
                    reason: AbsentReason::TurnedOff,
                },
                provenance: Provenance::Declared,
            }],
        },
        Case {
            name: "an on toggle changes nothing",
            claims: vec![claim(poll(), Provenance::Declared)],
            toggles: vec![KindToggle {
                kind: CapabilityKind::Storage,
                toggle: Toggle::On,
            }],
            expected: vec![claim(poll(), Provenance::Declared)],
        },
    ]
}

#[test]
fn effective_keeps_the_most_authoritative_claim_per_kind() {
    for case in cases() {
        assert_eq!(
            effective(&case.claims, &case.toggles),
            case.expected,
            "{}",
            case.name
        );
    }
}
