//! Every view, input and progress step of the sheet survives its serde form, and the forms other
//! programs read keep their slugs.

use porter_capability::CapabilityKind;
use porter_capability::{AbsentReason, KindToggle, Toggle};
use porter_core::SessionToken;
use porter_core::consent::{AccountChoice, ConsentAnswer, ConsentAsk, Usage};
use porter_core::{AccountId, ProviderId};
use porter_core::{
    AccountLabel, AppId, AppLabel, AppName, DataClass, EndpointUrl, Family, Isolation, LimitReason,
    LoginName, SecretText, ServiceEndpoint, Tls,
};
use porter_ids::WebUrl;
use porter_sheet::{
    Entry, FieldAnswer, FieldKind, FieldProblem, FieldSpec, FieldValue, MarkColour, MarkFace,
    MarkLetter, Presence, ProblemKind, Progress, ProviderGroup, ProviderKind, ProviderRow, Review,
    ReviewView, RowKind, ServiceRow, ServiceState, SheetInput, SheetView, SignInFault, SignInView,
    UserCode,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(&back, value, "{json}");
}

fn account_id(s: &str) -> AccountId {
    AccountId::parse(s).expect("account id")
}

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Photos").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn endpoints() -> Vec<ServiceEndpoint> {
    let url = |text: &str| EndpointUrl::parse(text).expect("url");
    vec![
        ServiceEndpoint {
            family: Family::Imap,
            url: url("imaps://imap.example.org"),
            tls: Tls::Implicit,
            login: LoginName("ada@example.org".into()),
        },
        ServiceEndpoint {
            family: Family::Smtp,
            url: url("smtp://smtp.example.org:587"),
            tls: Tls::StartTls,
            login: LoginName("ada".into()),
        },
        ServiceEndpoint {
            family: Family::WebDav,
            url: url("https://cloud.example.org/remote.php/dav/"),
            tls: Tls::Implicit,
            login: LoginName("ada".into()),
        },
        ServiceEndpoint {
            family: Family::Sieve,
            url: url("sieve://imap.example.org:4190"),
            tls: Tls::StartTls,
            login: LoginName("ada@example.org".into()),
        },
    ]
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializes")
}

#[test]
fn a_provider_row_names_its_group_only_when_it_has_one_and_an_old_row_reads_as_none() {
    let mut row = ProviderRow {
        id: ProviderId::parse("openai").expect("provider"),
        label: "OpenAI".into(),
        mark: "openai".into(),
        kind: RowKind::Provider,
        auth: ProviderKind::Service,
        mark_face: None,
        group: None,
    };
    assert!(!json(&row).contains("group"), "{}", json(&row));
    assert_eq!(
        serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
        row
    );
    // What the build before `group` wrote.
    let old =
        r#"{"id":"openai","label":"OpenAI","mark":"openai","kind":"provider","auth":"service"}"#;
    assert_eq!(
        serde_json::from_str::<ProviderRow>(old).expect("old").group,
        None
    );
    for (group, slug) in [
        (ProviderGroup::Internet, "internet"),
        (ProviderGroup::Intelligence, "intelligence"),
        (ProviderGroup::Agent, "agent"),
    ] {
        row.group = Some(group);
        assert!(
            json(&row).contains(&format!(r#""group":"{slug}""#)),
            "{}",
            json(&row)
        );
        assert_eq!(
            serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
            row
        );
    }
    let bad = r#"{"id":"openai","label":"OpenAI","mark":"openai","group":"cloud"}"#;
    assert!(serde_json::from_str::<ProviderRow>(bad).is_err());
}

#[test]
fn a_provider_row_names_its_face_only_when_it_has_one_and_an_old_row_reads_as_none() {
    let mut row = ProviderRow {
        id: ProviderId::parse("openai").expect("provider"),
        label: "OpenAI".into(),
        mark: "openai".into(),
        kind: RowKind::Provider,
        auth: ProviderKind::Service,
        mark_face: None,
        group: None,
    };
    assert!(!json(&row).contains("mark_face"), "{}", json(&row));
    assert_eq!(
        serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
        row
    );
    let old =
        r#"{"id":"openai","label":"OpenAI","mark":"openai","kind":"provider","auth":"service"}"#;
    assert_eq!(serde_json::from_str::<ProviderRow>(old).expect("old"), row);
    row.mark_face = Some(MarkFace {
        letter: MarkLetter::parse("O").expect("letter"),
        colour: MarkColour::parse("#10a37f").expect("colour"),
    });
    assert!(
        json(&row).contains(r##""mark_face":{"letter":"O","colour":"#10A37F"}"##),
        "{}",
        json(&row)
    );
    assert_eq!(
        serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
        row
    );
    let bad = r##"{"id":"openai","label":"OpenAI","mark":"openai","mark_face":{"letter":"OPE","colour":"#10A37F"}}"##;
    assert!(serde_json::from_str::<ProviderRow>(bad).is_err());
}

#[test]
fn a_sign_in_fault_keeps_the_slug_of_an_account_already_there() {
    assert_eq!(json(&SignInFault::AlreadyAdded), r#""already_added""#);
    // Vocabulary 12: the three faults a sheet words for the sign-in of a program on this computer.
    assert_eq!(json(&SignInFault::NotRunning), r#""not_running""#);
    assert_eq!(json(&SignInFault::SignedOut), r#""signed_out""#);
    assert_eq!(json(&SignInFault::NotAllowed), r#""not_allowed""#);
}

fn field(kind: FieldKind, entry: Entry) -> FieldSpec {
    FieldSpec {
        kind,
        entry,
        presence: Presence::Required,
        prefill: None,
    }
}

fn review() -> Review {
    Review {
        label: AccountLabel("ada@example.org".into()),
        services: vec![
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
        ],
        endpoints: endpoints(),
    }
}

#[test]
fn every_sheet_view_round_trips() {
    let nextcloud = ProviderId::parse("nextcloud").expect("provider");
    let url = EndpointUrl::parse("https://cloud.example.org/login/v2/flow/abc").expect("url");
    let row = Some(ProviderRow {
        id: nextcloud.clone(),
        label: "Nextcloud".into(),
        mark: "nextcloud".into(),
        kind: RowKind::Provider,
        auth: ProviderKind::Service,
        mark_face: None,
        group: Some(ProviderGroup::Internet),
    });
    let views = vec![
        SheetView::Consent(
            ConsentAsk::new(
                app(),
                CapabilityKind::Storage,
                DataClass::Photos,
                Usage::Interactive,
                vec![AccountChoice::new(
                    account_id("cloud"),
                    AccountLabel("Nextcloud".into()),
                    nextcloud.clone(),
                )],
            )
            .with_session(SessionToken::parse("sess-1").expect("session"))
            .with_app_label(AppLabel("Photos".into())),
        ),
        SheetView::Providers(vec![ProviderRow {
            id: nextcloud.clone(),
            label: "Nextcloud".into(),
            mark: "nextcloud".into(),
            kind: RowKind::Provider,
            auth: ProviderKind::AgentLogin,
            mark_face: Some(MarkFace {
                letter: MarkLetter::parse("Cx").expect("letter"),
                colour: MarkColour::parse("#10a37f").expect("colour"),
            }),
            group: Some(ProviderGroup::Agent),
        }]),
        SheetView::SignIn(SignInView {
            provider: nextcloud.clone(),
            row: row.clone(),
            fields: vec![
                field(FieldKind::Address, Entry::Plain),
                field(FieldKind::Password, Entry::Secret),
            ],
            problem: Some(FieldProblem {
                field: FieldKind::Password,
                problem: ProblemKind::Refused,
            }),
        }),
        SheetView::BrowserWait {
            provider: nextcloud.clone(),
            row: row.clone(),
            url: WebUrl::parse("https://login.example.org/authorize?state=abc").expect("url"),
        },
        SheetView::ShowCode {
            provider: nextcloud.clone(),
            row: row.clone(),
            user_code: UserCode("ABCD-EFGH".into()),
            url,
        },
        SheetView::Review(ReviewView {
            provider: nextcloud.clone(),
            row: row.clone(),
            review: review(),
            allow: Some(app()),
            allow_label: Some(AppLabel("Photos".into())),
        }),
        SheetView::Working {
            provider: nextcloud.clone(),
            row: row.clone(),
        },
        SheetView::Working {
            provider: nextcloud.clone(),
            row: None,
        },
        SheetView::Failed {
            provider: nextcloud,
            row,
            fault: SignInFault::NeedsClientId,
        },
        SheetView::Done,
    ];
    views.iter().for_each(round_trip);
}

#[test]
fn every_sheet_input_and_progress_round_trips() {
    let inputs = [
        SheetInput::Answer(ConsentAnswer::Deny),
        SheetInput::Pick(ProviderId::parse("nextcloud").expect("provider")),
        SheetInput::Submit(vec![
            FieldAnswer {
                kind: FieldKind::Address,
                value: FieldValue::Plain("ada@example.org".into()),
            },
            FieldAnswer {
                kind: FieldKind::Password,
                value: FieldValue::Secret(SecretText::new("pw")),
            },
        ]),
        SheetInput::Confirm(vec![KindToggle {
            kind: CapabilityKind::Notes,
            toggle: Toggle::Off,
        }]),
        SheetInput::Back,
        SheetInput::Retry,
        SheetInput::OpenAgain,
        SheetInput::Dismiss,
    ];
    inputs.iter().for_each(round_trip);
    assert_eq!(
        serde_json::to_string(&SheetInput::OpenAgain).expect("json"),
        r#"{"kind":"open_again"}"#
    );
    // A row from a host that predates `kind` is a provider row.
    let old: ProviderRow =
        serde_json::from_str(r#"{"id":"nextcloud","label":"Nextcloud","mark":"nextcloud"}"#)
            .expect("row");
    assert_eq!(old.kind, RowKind::Provider);
    assert_eq!(
        serde_json::to_string(&RowKind::Generic).expect("json"),
        r#""generic""#
    );
    let url = EndpointUrl::parse("https://login.example.org/device").expect("url");
    let steps = [
        Progress::Ask(vec![field(FieldKind::ApiKey, Entry::Secret)]),
        Progress::Browser(
            WebUrl::parse("https://login.example.org/authorize?client_id=a&scope=b%20c")
                .expect("url"),
        ),
        Progress::Code {
            user_code: UserCode("ABCD".into()),
            url,
        },
        Progress::Waiting,
        Progress::Review(review()),
        Progress::Done,
        Progress::Failed(SignInFault::TimedOut),
        Progress::Failed(SignInFault::StoreFailed),
    ];
    steps.iter().for_each(round_trip);
}

#[test]
fn a_typed_password_is_redacted_in_debug_and_present_on_the_wire() {
    let input = SheetInput::Submit(vec![FieldAnswer {
        kind: FieldKind::Password,
        value: FieldValue::Secret(SecretText::new("hunter2")),
    }]);
    assert!(!format!("{input:?}").contains("hunter2"));
    assert_eq!(
        json(&input),
        r#"{"kind":"submit","v":[{"kind":"password","value":{"kind":"secret","v":"hunter2"}}]}"#
    );
}

#[test]
fn a_provider_row_and_views_written_before_the_kind_and_name_still_read() {
    let old_row = r#"{"id":"claude-code","label":"Claude Code","mark":"claude","kind":"provider"}"#;
    let row: ProviderRow = serde_json::from_str(old_row).expect("old row");
    assert_eq!(row.auth, ProviderKind::Service);
    let old_view = r#"{"kind":"browser_wait","v":{"provider":"nextcloud","url":"https://login.example.org/a"}}"#;
    match serde_json::from_str::<SheetView>(old_view).expect("old view") {
        SheetView::BrowserWait { row, .. } => assert_eq!(row, None),
        other => panic!("{other:?}"),
    }
    let agent = r#"{"id":"claude-code","label":"Claude Code","mark":"c","auth":"agent_login"}"#;
    let row: ProviderRow = serde_json::from_str(agent).expect("row");
    assert_eq!(
        (row.auth, row.label.as_str()),
        (ProviderKind::AgentLogin, "Claude Code")
    );
}
