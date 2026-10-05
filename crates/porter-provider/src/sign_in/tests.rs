use super::*;
use porter_core::capability::{
    Access, Capability, CapabilityKind, Delta, HashKind, Offered, QuotaReport, StorageCap,
    StorageScope,
};
use porter_core::{AbsentReason, Limit, LimitReason, Provenance, SecretText, Subject};

fn storage_claim() -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(Capability::Storage(StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: QuotaReport::Reported,
            scope: StorageScope::AppFolder,
            hashes: HashKind::None,
            ranges: Offered::Present,
            chunked_upload: Offered::Absent,
        })),
        provenance: Provenance::Discovered,
    }
}

fn notes_missing() -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Absent {
            kind: CapabilityKind::Notes,
            reason: AbsentReason::NotOnServer,
        },
        provenance: Provenance::Probed,
    }
}

#[test]
fn a_review_step_tells_the_sheet_each_service_and_its_limit() {
    let restriction = Restriction {
        limits: vec![Limit {
            kind: CapabilityKind::Storage,
            reason: LimitReason::AppFolderOnly,
        }],
        ..Restriction::none()
    };
    let step = SignInStep::Review {
        claims: vec![storage_claim(), notes_missing()],
        endpoints: vec![],
        restriction,
        label: AccountLabel("ada@example.org".into()),
    };
    let Progress::Review(review) = step.progress() else {
        panic!("not a review");
    };
    assert_eq!(
        review.services,
        vec![
            ServiceRow {
                kind: CapabilityKind::Storage,
                state: ServiceState::Offered(Toggle::On),
                limit: Some(LimitReason::AppFolderOnly),
            },
            ServiceRow {
                kind: CapabilityKind::Notes,
                state: ServiceState::Absent(AbsentReason::NotOnServer),
                limit: None,
            },
        ]
    );
}

#[test]
fn what_the_sheet_is_told_of_a_step_never_holds_the_credential() {
    let signed = Signed {
        label: AccountLabel("ada@example.org".into()),
        credentials: vec![(
            SecretPurpose::Password,
            Credential::Password(SecretText::new("app-pw-xyz")),
        )],
        claims: vec![],
        endpoints: vec![],
        restriction: Restriction::none(),
    };
    assert!(!format!("{signed:?}").contains("xyz"));
    let progress = SignInStep::Done(signed).progress();
    assert_eq!(progress, Progress::Done);
    assert!(!format!("{progress:?}").contains("xyz"));
}

#[test]
fn the_other_steps_pass_through() {
    let url = EndpointUrl::parse("https://cloud.example.org/login/v2/flow").expect("url");
    let code = UserCode("ABCD-EFGH".into());
    let cases = [
        (SignInStep::Waiting, Progress::Waiting),
        (
            SignInStep::Failed(SignInFault::Refused),
            Progress::Failed(SignInFault::Refused),
        ),
        (
            SignInStep::OpenBrowser { url: url.clone() },
            Progress::Browser(url.clone()),
        ),
        (
            SignInStep::ShowCode {
                user_code: code.clone(),
                url: url.clone(),
            },
            Progress::Code {
                user_code: code,
                url,
            },
        ),
    ];
    for (step, expected) in cases {
        assert_eq!(step.progress(), expected);
    }
}
