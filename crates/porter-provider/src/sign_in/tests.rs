use super::*;
use porter_capability::{AbsentReason, Provenance, Subject};
use porter_capability::{
    Access, Capability, CapabilityKind, Delta, HashKind, StorageCap, StorageScope,
};
use porter_core::{Limit, LimitReason, SecretText};

fn storage_claim() -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(Capability::Storage(StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: true,
            scope: StorageScope::AppFolder,
            hashes: HashKind::None,
            ranges: true,
            chunked_upload: false,
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
    let restriction = Restriction::none().with_limits(vec![Limit::new(
        CapabilityKind::Storage,
        LimitReason::AppFolderOnly,
    )]);
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
                access: None,
            },
            ServiceRow {
                kind: CapabilityKind::Notes,
                state: ServiceState::Absent(AbsentReason::NotOnServer),
                limit: None,
                access: None,
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
    let page = WebUrl::parse("https://login.example.org/authorize?state=a&scope=b").expect("url");
    let code = UserCode("ABCD-EFGH".into());
    let cases = [
        (SignInStep::Waiting, Progress::Waiting),
        (
            SignInStep::Failed(SignInFault::Refused),
            Progress::Failed(SignInFault::Refused),
        ),
        (
            SignInStep::OpenBrowser { url: page.clone() },
            Progress::Browser(page),
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

#[test]
fn a_login_hint_is_one_address_or_a_fault() {
    let cases = [
        ("ada@gmail.com", Ok("ada@gmail.com")),
        ("  ada@gmail.com ", Ok("ada@gmail.com")),
        ("", Err(LoginHintFault::Empty)),
        ("   ", Err(LoginHintFault::Empty)),
        ("ada", Err(LoginHintFault::NotAnAddress)),
        ("@gmail.com", Err(LoginHintFault::NotAnAddress)),
        ("ada@", Err(LoginHintFault::NotAnAddress)),
        ("ada lovelace@gmail.com", Err(LoginHintFault::NotAnAddress)),
        (
            "ada@gmail.com\n&prompt=none",
            Err(LoginHintFault::NotAnAddress),
        ),
    ];
    for (text, want) in cases {
        let got = LoginHint::parse(text);
        assert_eq!(
            got.as_ref().map(LoginHint::as_str),
            want.as_ref().map(|s| *s),
            "{text:?}"
        );
    }
}

#[test]
fn a_sign_in_starts_without_a_hint_and_with_a_first_consent() {
    let start = SignInStart::new(SignInMode::Add);
    assert_eq!(start.login_hint, None);
    assert_eq!(start.consent, ConsentHistory::FirstSignIn);
    let hint = LoginHint::parse("ada@gmail.com").expect("an address");
    let again = start
        .with_login_hint(hint.clone())
        .with_consent(ConsentHistory::SignedInBefore);
    assert_eq!(again.login_hint, Some(hint));
    assert_eq!(again.consent, ConsentHistory::SignedInBefore);
}
