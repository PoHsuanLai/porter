//! The machine's tables. The rules are those of mailo's add-account tests
//! (`mailo/crates/mail-app/src/ui/add_account/flow_tests.rs`, copied in spirit: looked up once,
//! nothing added without being told, checked field by field, a refusal says so and keeps going,
//! a password goes to the sign-in and nowhere else), restated over porter's stages.

use super::*;
use crate::account::AccountLabel;
use crate::capability::CapabilityKind;
use crate::effective::Toggle;
use crate::secret::SecretText;
use crate::sheet::fields::{Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence};
use crate::sheet::input::SheetInput;
use crate::sheet::progress::{
    Progress, Review, ServiceChoice, ServiceRow, ServiceState, SignInFault, SignInInput, UserCode,
};
use crate::sheet::view::{
    FieldProblem, ProblemKind, ProviderKind, ProviderRow, RowKind, SheetView,
};
use crate::wire::ProviderHint;
use proptest::prelude::*;

const SECRET: &str = "hunter2-correct-horse";

fn provider(text: &str) -> ProviderId {
    ProviderId::parse(text).expect("provider id")
}

fn nc() -> ProviderId {
    provider("nextcloud")
}

fn rows() -> Vec<ProviderRow> {
    ["nextcloud", "generic-imap"]
        .into_iter()
        .map(|id| ProviderRow {
            id: provider(id),
            label: id.into(),
            mark: id.into(),
            kind: match id.starts_with("generic-") {
                true => RowKind::Generic,
                false => RowKind::Provider,
            },
            auth: ProviderKind::Service,
            mark_face: None,
            group: None,
        })
        .collect()
}

fn url() -> EndpointUrl {
    EndpointUrl::parse("https://cloud.example.org/login/v2/flow").expect("url")
}

fn page() -> WebUrl {
    WebUrl::parse("https://login.example.org/authorize?client_id=a&state=b").expect("url")
}

fn spec(kind: FieldKind, entry: Entry, presence: Presence) -> FieldSpec {
    FieldSpec {
        kind,
        entry,
        presence,
        prefill: None,
    }
}

fn form() -> Vec<FieldSpec> {
    vec![
        spec(FieldKind::Address, Entry::Plain, Presence::Required),
        spec(FieldKind::Username, Entry::Plain, Presence::Optional),
        spec(FieldKind::Password, Entry::Secret, Presence::Required),
    ]
}

fn review() -> Review {
    Review {
        label: AccountLabel("ada@cloud.example.org".into()),
        services: vec![ServiceRow {
            kind: CapabilityKind::Storage,
            state: ServiceState::Offered(Toggle::On),
            limit: None,
        }],
        endpoints: vec![],
    }
}

fn choices() -> Vec<ServiceChoice> {
    vec![ServiceChoice {
        kind: CapabilityKind::Storage,
        toggle: Toggle::Off,
    }]
}

fn add() -> Purpose {
    Purpose::Add {
        hint: ProviderHint::Any,
        allow: None,
    }
}

fn reauth() -> Purpose {
    Purpose::Reauthenticate {
        account: AccountId::parse("cloud").expect("id"),
        provider: nc(),
    }
}

fn at(purpose: Purpose, stage: Stage) -> Sheet {
    Sheet {
        purpose,
        stage,
        providers: rows(),
    }
}

fn asking(problem: Option<FieldProblem>) -> Stage {
    Stage::Asking {
        provider: nc(),
        fields: form(),
        problem,
    }
}

fn browser() -> Stage {
    Stage::Browser {
        provider: nc(),
        url: page(),
    }
}

fn code() -> Stage {
    Stage::Code {
        provider: nc(),
        user_code: UserCode("ABCD-EFGH".into()),
        url: url(),
    }
}

fn reviewing() -> Stage {
    Stage::Reviewing {
        provider: nc(),
        review: review(),
    }
}

fn confirming() -> Stage {
    Stage::Confirming {
        provider: nc(),
        choices: choices(),
    }
}

fn failed(fault: SignInFault) -> Stage {
    Stage::Failed {
        provider: nc(),
        fault,
    }
}

fn every_stage() -> Vec<(&'static str, Stage)> {
    vec![
        ("choosing", Stage::Choosing(rows())),
        ("asking", asking(None)),
        ("working", Stage::Working(nc())),
        ("browser", browser()),
        ("code", code()),
        ("reviewing", reviewing()),
        ("confirming", confirming()),
        ("added", Stage::Added),
        ("failed", failed(SignInFault::Refused)),
    ]
}

fn plain(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Plain(text.into()),
    }
}

fn secret(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Secret(SecretText::new(text)),
    }
}

fn run(sheet: Sheet, event: SheetEvent) -> (Stage, Vec<SheetEffect>) {
    let (sheet, effects) = step(sheet, event);
    (sheet.stage, effects)
}

fn shown(stage: Stage) -> SheetEffect {
    SheetEffect::Show(at(add(), stage).view())
}

fn input(i: SheetInput) -> SheetEvent {
    SheetEvent::Input(i)
}

#[test]
fn a_pick_starts_the_sign_in_once() {
    let (stage, effects) = run(
        at(add(), Stage::Choosing(rows())),
        input(SheetInput::Pick(nc())),
    );
    assert_eq!(stage, Stage::Working(nc()));
    assert_eq!(
        effects,
        vec![
            SheetEffect::Show(SheetView::Working {
                provider: nc(),
                row: Some(rows()[0].clone()),
            }),
            SheetEffect::Feed(SignInInput::Start)
        ]
    );
    // A second press while it works starts nothing.
    let (stage, effects) = run(at(add(), stage), input(SheetInput::Pick(nc())));
    assert_eq!((stage, effects), (Stage::Working(nc()), vec![]));
}

#[test]
fn a_pick_of_a_provider_not_in_the_list_is_dropped() {
    let (stage, effects) = run(
        at(add(), Stage::Choosing(rows())),
        input(SheetInput::Pick(provider("google"))),
    );
    assert_eq!((stage, effects), (Stage::Choosing(rows()), vec![]));
}

#[test]
fn progress_moves_a_working_sheet_to_what_the_sign_in_needs() {
    let cases = [
        ("ask", Progress::Ask(form()), asking(None)),
        ("browser", Progress::Browser(page()), browser()),
        (
            "code",
            Progress::Code {
                user_code: UserCode("ABCD-EFGH".into()),
                url: url(),
            },
            code(),
        ),
        ("review", Progress::Review(review()), reviewing()),
        (
            "refused",
            Progress::Failed(SignInFault::Refused),
            failed(SignInFault::Refused),
        ),
        (
            "timed out",
            Progress::Failed(SignInFault::TimedOut),
            failed(SignInFault::TimedOut),
        ),
    ];
    for (name, progress, want) in cases {
        let (stage, effects) = run(
            at(add(), Stage::Working(nc())),
            SheetEvent::SignIn(progress),
        );
        assert_eq!(stage, want, "{name}");
        assert_eq!(effects, vec![shown(want)], "{name}");
    }
}

#[test]
fn the_browser_and_code_waits_poll_until_the_sign_in_moves() {
    for (name, stage) in [
        ("browser", browser()),
        ("code", code()),
        ("working", Stage::Working(nc())),
    ] {
        let (after, effects) = run(
            at(add(), stage.clone()),
            SheetEvent::SignIn(Progress::Waiting),
        );
        assert_eq!(after, stage, "{name}");
        assert_eq!(
            effects,
            vec![SheetEffect::Feed(SignInInput::Poll)],
            "{name}"
        );
    }
    let (after, effects) = run(
        at(add(), browser()),
        SheetEvent::SignIn(Progress::Review(review())),
    );
    assert_eq!((after, effects), (reviewing(), vec![shown(reviewing())]));
}

#[test]
fn a_review_offered_to_a_sign_in_again_is_confirmed_unchanged_not_shown() {
    for (name, stage, shows) in [
        ("working", Stage::Working(nc()), vec![]),
        (
            "browser",
            browser(),
            vec![SheetEffect::Show(SheetView::Working {
                provider: nc(),
                row: Some(rows()[0].clone()),
            })],
        ),
        (
            "code",
            code(),
            vec![SheetEffect::Show(SheetView::Working {
                provider: nc(),
                row: Some(rows()[0].clone()),
            })],
        ),
    ] {
        let (after, effects) = run(
            at(reauth(), stage),
            SheetEvent::SignIn(Progress::Review(review())),
        );
        assert_eq!(
            after,
            Stage::Confirming {
                provider: nc(),
                choices: vec![]
            },
            "{name}"
        );
        let confirm = SheetEffect::Feed(SignInInput::Confirm(vec![]));
        assert_eq!(effects, [shows, vec![confirm]].concat(), "{name}");
        let (_, effects) = run(at(reauth(), after), SheetEvent::SignIn(Progress::Done));
        assert_eq!(effects, vec![SheetEffect::Store(vec![])], "{name}");
    }
}

#[test]
fn progress_that_does_not_fit_the_stage_is_dropped() {
    for (name, stage) in every_stage() {
        if matches!(
            stage,
            Stage::Working(_)
                | Stage::Confirming { .. }
                | Stage::Browser { .. }
                | Stage::Code { .. }
        ) {
            continue;
        }
        let (after, effects) = run(
            at(add(), stage.clone()),
            SheetEvent::SignIn(Progress::Waiting),
        );
        assert_eq!((after, effects), (stage, vec![]), "{name}");
    }
}

#[test]
fn a_form_is_checked_field_by_field_and_nothing_leaves_until_it_is_whole() {
    let cases = [
        ("nothing typed", vec![], FieldKind::Address),
        (
            "blank address",
            vec![
                plain(FieldKind::Address, "  "),
                secret(FieldKind::Password, SECRET),
            ],
            FieldKind::Address,
        ),
        (
            "no password",
            vec![plain(FieldKind::Address, "ada@cloud.example.org")],
            FieldKind::Password,
        ),
        (
            "empty password",
            vec![
                plain(FieldKind::Address, "ada@cloud.example.org"),
                secret(FieldKind::Password, ""),
            ],
            FieldKind::Password,
        ),
    ];
    for (name, answers, field) in cases {
        let (stage, effects) = run(at(add(), asking(None)), input(SheetInput::Submit(answers)));
        let want = asking(Some(FieldProblem {
            field,
            problem: ProblemKind::Missing,
        }));
        assert_eq!(stage, want, "{name}");
        assert_eq!(effects, vec![shown(want)], "{name}: no Feed");
    }
}

#[test]
fn a_whole_form_goes_to_the_sign_in_trimmed_and_only_what_was_asked() {
    let answers = vec![
        secret(FieldKind::Password, SECRET),
        plain(FieldKind::Address, " ada@cloud.example.org "),
        plain(FieldKind::Server, "evil.example.org"),
    ];
    let (stage, effects) = run(at(add(), asking(None)), input(SheetInput::Submit(answers)));
    assert_eq!(stage, Stage::Working(nc()));
    assert_eq!(
        effects,
        vec![
            SheetEffect::Show(SheetView::Working {
                provider: nc(),
                row: Some(rows()[0].clone()),
            }),
            SheetEffect::Feed(SignInInput::Fields(vec![
                plain(FieldKind::Address, "ada@cloud.example.org"),
                secret(FieldKind::Password, SECRET),
            ])),
        ]
    );
}

#[test]
fn a_form_can_be_resubmitted_after_a_problem_is_marked() {
    let marked = asking(Some(FieldProblem {
        field: FieldKind::Password,
        problem: ProblemKind::Missing,
    }));
    let answers = vec![
        plain(FieldKind::Address, "ada@cloud.example.org"),
        secret(FieldKind::Password, SECRET),
    ];
    let (stage, _) = run(at(add(), marked), input(SheetInput::Submit(answers)));
    assert_eq!(stage, Stage::Working(nc()));
}

#[test]
fn confirm_stores_nothing_and_done_stores_the_choices() {
    let (stage, effects) = run(
        at(add(), reviewing()),
        input(SheetInput::Confirm(choices())),
    );
    assert_eq!(stage, confirming());
    assert_eq!(
        effects,
        vec![
            SheetEffect::Show(SheetView::Working {
                provider: nc(),
                row: Some(rows()[0].clone()),
            }),
            SheetEffect::Feed(SignInInput::Confirm(choices())),
        ],
        "no Store before Done"
    );
    let (stage, effects) = run(at(add(), stage), SheetEvent::SignIn(Progress::Done));
    assert_eq!(stage, confirming());
    assert_eq!(effects, vec![SheetEffect::Store(choices())]);
    // Reaching the review, or a sign-in that says Done with nothing confirmed, stores nothing.
    let (_, effects) = run(
        at(add(), Stage::Working(nc())),
        SheetEvent::SignIn(Progress::Done),
    );
    assert_eq!(effects, vec![]);
    let (_, effects) = run(at(add(), reviewing()), SheetEvent::SignIn(Progress::Done));
    assert_eq!(effects, vec![]);
    for (name, stage) in every_stage() {
        if matches!(stage, Stage::Reviewing { .. }) {
            continue;
        }
        let (_, effects) = run(at(add(), stage), input(SheetInput::Confirm(choices())));
        assert!(effects.is_empty(), "{name}: Confirm outside the review");
    }
}

#[test]
fn a_sign_in_that_fails_after_confirm_stores_nothing() {
    for fault in [
        SignInFault::Refused,
        SignInFault::TimedOut,
        SignInFault::Unreachable,
    ] {
        let (stage, effects) = run(
            at(add(), confirming()),
            SheetEvent::SignIn(Progress::Failed(fault)),
        );
        assert_eq!(stage, failed(fault));
        assert_eq!(effects, vec![shown(failed(fault))], "{fault:?}: no Store");
    }
    // Questions and reviews after Confirm are dropped.
    let (stage, effects) = run(
        at(add(), confirming()),
        SheetEvent::SignIn(Progress::Review(review())),
    );
    assert_eq!((stage, effects), (confirming(), vec![]));
}

#[test]
fn add_and_allow_stores_the_same_way_and_the_view_names_the_app() {
    let app = crate::AppId {
        name: crate::AppName::parse("org.quire.Mail").expect("name"),
        isolation: crate::Isolation::Flatpak,
    };
    let purpose = Purpose::Add {
        hint: ProviderHint::Any,
        allow: Some(app.clone()),
    };
    let sheet = at(purpose, Stage::Working(nc()));
    let (sheet, effects) = step(sheet, SheetEvent::SignIn(Progress::Review(review())));
    match &effects[..] {
        [SheetEffect::Show(SheetView::Review(view))] => assert_eq!(view.allow, Some(app)),
        other => panic!("not the review: {other:?}"),
    }
    let (sheet, _) = step(sheet, input(SheetInput::Confirm(choices())));
    let (_, effects) = step(sheet, SheetEvent::SignIn(Progress::Done));
    assert_eq!(
        effects,
        vec![SheetEffect::Store(choices())],
        "one store, one grant"
    );
}

#[test]
fn a_re_sign_in_stores_on_done_and_never_on_confirm() {
    // Working, and the stages a page or a code leaves the sheet in: all store.
    for (name, stage) in [
        ("working", Stage::Working(nc())),
        ("browser", browser()),
        ("code", code()),
    ] {
        let (_, effects) = run(at(reauth(), stage), SheetEvent::SignIn(Progress::Done));
        assert_eq!(effects, vec![SheetEffect::Store(vec![])], "{name}");
    }
    // A new account is stored only after Confirm, never from a page.
    for (name, stage) in [("browser", browser()), ("code", code())] {
        let (_, effects) = run(at(add(), stage), SheetEvent::SignIn(Progress::Done));
        assert_eq!(effects, vec![], "{name}: add");
    }
    let (_, effects) = run(
        at(reauth(), reviewing()),
        input(SheetInput::Confirm(vec![])),
    );
    assert!(!effects.iter().any(|e| matches!(e, SheetEffect::Store(_))));
}

#[test]
fn stored_shows_done_and_closes_and_a_failed_store_fails_the_sheet() {
    let (stage, effects) = run(at(add(), Stage::Working(nc())), SheetEvent::Stored);
    assert_eq!(stage, Stage::Added);
    assert_eq!(
        effects,
        vec![
            SheetEffect::Show(SheetView::Done),
            SheetEffect::Close(SheetEnd::Added)
        ]
    );
    let (stage, effects) = run(at(add(), Stage::Working(nc())), SheetEvent::StoreFailed);
    assert_eq!(stage, failed(SignInFault::StoreFailed));
    assert_eq!(effects, vec![shown(failed(SignInFault::StoreFailed))]);
    let (stage, _) = run(at(add(), confirming()), SheetEvent::Stored);
    assert_eq!(stage, Stage::Added);
    for (name, stage) in every_stage() {
        if matches!(stage, Stage::Working(_) | Stage::Confirming { .. }) {
            continue;
        }
        for event in [SheetEvent::Stored, SheetEvent::StoreFailed] {
            let (after, effects) = run(at(add(), stage.clone()), event);
            assert_eq!((after, effects), (stage.clone(), vec![]), "{name}");
        }
    }
}

#[test]
fn a_stored_re_sign_in_closes_the_sheet_from_the_page_and_the_code() {
    for (name, stage) in [("browser", browser()), ("code", code())] {
        let (after, effects) = run(at(reauth(), stage.clone()), SheetEvent::Stored);
        assert_eq!(after, Stage::Added, "{name}");
        assert_eq!(
            effects,
            vec![
                SheetEffect::Show(SheetView::Done),
                SheetEffect::Close(SheetEnd::Added)
            ],
            "{name}"
        );
        let (after, effects) = run(at(reauth(), stage.clone()), SheetEvent::StoreFailed);
        assert_eq!(after, failed(SignInFault::StoreFailed), "{name}");
        assert_eq!(
            effects,
            vec![shown(failed(SignInFault::StoreFailed))],
            "{name}"
        );
        // Adding never stores from a page, so a store answer there is stale.
        for event in [SheetEvent::Stored, SheetEvent::StoreFailed] {
            let (after, effects) = run(at(add(), stage.clone()), event);
            assert_eq!((after, effects), (stage.clone(), vec![]), "{name}: add");
        }
    }
}

#[test]
fn retry_starts_over_unless_asking_again_cannot_help() {
    for fault in [
        SignInFault::Refused,
        SignInFault::Unreachable,
        SignInFault::TimedOut,
        SignInFault::Cancelled,
    ] {
        let (stage, effects) = run(at(add(), failed(fault)), input(SheetInput::Retry));
        assert_eq!(stage, Stage::Working(nc()), "{fault:?}");
        assert_eq!(
            effects.last(),
            Some(&SheetEffect::Feed(SignInInput::Start)),
            "{fault:?}"
        );
    }
    for fault in [SignInFault::Forbidden, SignInFault::NeedsClientId] {
        let (stage, effects) = run(at(add(), failed(fault)), input(SheetInput::Retry));
        assert_eq!((stage, effects), (failed(fault), vec![]), "{fault:?}");
    }
}

#[test]
fn back_returns_to_the_list_where_the_person_chose_from_it() {
    for (name, stage) in [
        ("form", asking(None)),
        ("browser", browser()),
        ("code", code()),
        ("review", reviewing()),
    ] {
        let (after, effects) = run(at(add(), stage), input(SheetInput::Back));
        assert_eq!(after, Stage::Choosing(rows()), "{name}");
        assert_eq!(
            effects,
            vec![
                SheetEffect::Show(SheetView::Providers(rows())),
                SheetEffect::Feed(SignInInput::Cancel)
            ],
            "{name}"
        );
    }
    let (after, effects) = run(
        at(add(), failed(SignInFault::Refused)),
        input(SheetInput::Back),
    );
    assert_eq!(after, Stage::Choosing(rows()));
    assert_eq!(
        effects,
        vec![SheetEffect::Show(SheetView::Providers(rows()))],
        "nothing runs"
    );
    for stage in [
        Stage::Choosing(rows()),
        Stage::Working(nc()),
        confirming(),
        Stage::Added,
    ] {
        let (after, effects) = run(at(add(), stage.clone()), input(SheetInput::Back));
        assert_eq!((after, effects), (stage, vec![]));
    }
}

#[test]
fn back_starts_a_known_provider_over() {
    let hinted = Purpose::Add {
        hint: ProviderHint::Provider(nc()),
        allow: None,
    };
    for purpose in [hinted, reauth()] {
        let (after, effects) = run(at(purpose.clone(), reviewing()), input(SheetInput::Back));
        assert_eq!(after, Stage::Working(nc()));
        assert_eq!(
            effects[1..],
            [
                SheetEffect::Feed(SignInInput::Cancel),
                SheetEffect::Feed(SignInInput::Start)
            ]
        );
        let (_, effects) = run(
            at(purpose, failed(SignInFault::Refused)),
            input(SheetInput::Back),
        );
        assert_eq!(effects[1..], [SheetEffect::Feed(SignInInput::Start)]);
    }
}

#[test]
fn every_stage_can_be_dismissed_and_a_running_sign_in_is_cancelled() {
    for (name, stage) in every_stage() {
        for event in [input(SheetInput::Dismiss), SheetEvent::Left] {
            let running = matches!(
                stage,
                Stage::Working(_)
                    | Stage::Confirming { .. }
                    | Stage::Asking { .. }
                    | Stage::Browser { .. }
                    | Stage::Code { .. }
                    | Stage::Reviewing { .. }
            );
            let end = match &stage {
                Stage::Failed { fault, .. } => SheetEnd::Failed(*fault),
                _ => SheetEnd::Dismissed,
            };
            let (after, effects) = run(at(add(), stage.clone()), event);
            assert_eq!(after, stage, "{name}: the stage is left as it was");
            let want = match &stage {
                Stage::Added => vec![],
                _ if running => vec![
                    SheetEffect::Feed(SignInInput::Cancel),
                    SheetEffect::Close(end),
                ],
                _ => vec![SheetEffect::Close(end)],
            };
            assert_eq!(effects, want, "{name}");
        }
    }
}

#[test]
fn a_consent_answer_is_not_the_machines_input() {
    for (name, stage) in every_stage() {
        let event = input(SheetInput::Answer(crate::consent::ConsentAnswer::Deny));
        let (after, effects) = run(at(add(), stage.clone()), event);
        assert_eq!((after, effects), (stage, vec![]), "{name}");
    }
}

#[test]
fn open_again_on_the_browser_page_opens_it_again_and_does_not_restart() {
    let sheet = at(add(), browser());
    let (after, effects) = step(sheet.clone(), input(SheetInput::OpenAgain));
    assert_eq!(after, sheet);
    assert_eq!(effects, vec![SheetEffect::OpenBrowser(page())]);
    // Again, and again: the same, and nothing is cancelled or fed.
    let (after, effects) = step(after, input(SheetInput::OpenAgain));
    assert_eq!(after, sheet);
    assert_eq!(effects, vec![SheetEffect::OpenBrowser(page())]);
}

#[test]
fn open_again_means_nothing_off_the_browser_page() {
    for (name, stage) in every_stage() {
        if matches!(stage, Stage::Browser { .. }) {
            continue;
        }
        let (after, effects) = run(at(add(), stage.clone()), input(SheetInput::OpenAgain));
        assert_eq!((after, effects), (stage, vec![]), "{name}");
    }
}

#[test]
fn the_generic_rows_of_the_list_are_marked_generic_in_the_view() {
    let SheetView::Providers(rows) = at(add(), Stage::Choosing(rows())).view() else {
        panic!("the list");
    };
    let kinds: Vec<_> = rows.iter().map(|r| r.kind).collect();
    assert_eq!(kinds, vec![RowKind::Provider, RowKind::Generic]);
}

#[test]
fn the_device_code_flow_from_start_to_stored() {
    let sheet = Sheet::new(
        Purpose::Add {
            hint: ProviderHint::Provider(provider("microsoft")),
            allow: None,
        },
        rows(),
    );
    let ms = provider("microsoft");
    let events = [
        SheetEvent::SignIn(Progress::Code {
            user_code: UserCode("ABCD-EFGH".into()),
            url: url(),
        }),
        SheetEvent::SignIn(Progress::Waiting),
        SheetEvent::SignIn(Progress::Review(review())),
        input(SheetInput::Confirm(vec![])),
        SheetEvent::Stored,
    ];
    let mut all = vec![];
    let mut sheet = sheet;
    for event in events {
        let (next, effects) = step(sheet, event);
        sheet = next;
        all.extend(effects);
    }
    assert_eq!(sheet.stage, Stage::Added);
    assert_eq!(all.last(), Some(&SheetEffect::Close(SheetEnd::Added)));
    assert!(all.contains(&SheetEffect::Feed(SignInInput::Poll)));
    assert!(all.contains(&SheetEffect::Show(SheetView::ShowCode {
        provider: ms,
        row: None,
        user_code: UserCode("ABCD-EFGH".into()),
        url: url()
    })));
}

// The property: whatever the person and the sign-in do, in any order, a typed secret appears in
// no effect but `Feed(Fields)`, and there only under a field the form asked for.

fn leaks(effect: &SheetEffect) -> bool {
    let text = match effect {
        SheetEffect::Show(view) => serde_json::to_string(view).expect("view"),
        SheetEffect::Store(choices) => serde_json::to_string(choices).expect("choices"),
        SheetEffect::Close(end) => format!("{end:?}"),
        SheetEffect::OpenBrowser(url) => format!("{url:?}"),
        SheetEffect::Feed(SignInInput::Fields(_)) => return false,
        SheetEffect::Feed(other) => format!("{other:?}"),
    };
    text.contains(SECRET)
}

fn event_strategy() -> impl Strategy<Value = SheetEvent> {
    let secret_answers = || {
        vec![
            plain(FieldKind::Address, "ada@cloud.example.org"),
            secret(FieldKind::Password, SECRET),
            secret(FieldKind::ApiKey, SECRET),
        ]
    };
    prop_oneof![
        Just(input(SheetInput::Pick(nc()))),
        Just(input(SheetInput::OpenAgain)),
        Just(input(SheetInput::Submit(secret_answers()))),
        Just(input(SheetInput::Submit(vec![secret(
            FieldKind::Password,
            SECRET
        )]))),
        Just(input(SheetInput::Confirm(choices()))),
        Just(SheetEvent::SignIn(Progress::Failed(SignInFault::TimedOut))),
        Just(input(SheetInput::Back)),
        Just(input(SheetInput::Retry)),
        Just(input(SheetInput::Dismiss)),
        Just(SheetEvent::SignIn(Progress::Ask(form()))),
        Just(SheetEvent::SignIn(Progress::Browser(page()))),
        Just(SheetEvent::SignIn(Progress::Waiting)),
        Just(SheetEvent::SignIn(Progress::Review(review()))),
        Just(SheetEvent::SignIn(Progress::Done)),
        Just(SheetEvent::SignIn(Progress::Failed(SignInFault::Refused))),
        Just(SheetEvent::Stored),
        Just(SheetEvent::StoreFailed),
        Just(SheetEvent::Left),
    ]
}

proptest! {
    #[test]
    fn no_effect_carries_a_secret_outward(
        events in proptest::collection::vec(event_strategy(), 0..40),
        reauthenticating in any::<bool>(),
    ) {
        let purpose = if reauthenticating { reauth() } else { add() };
        let mut sheet = Sheet::new(purpose, rows());
        for event in events {
            let (next, effects) = step(sheet, event);
            for effect in &effects {
                prop_assert!(!leaks(effect), "{effect:?}");
                if let SheetEffect::Feed(SignInInput::Fields(answers)) = effect {
                    prop_assert!(answers.iter().all(|a| a.kind != FieldKind::ApiKey), "only asked fields");
                }
            }
            sheet = next;
        }
    }
}
