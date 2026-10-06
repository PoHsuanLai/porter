//! The server form through the machine: asked when a lookup finds nothing, reshaped by its
//! protocol, checked before anything is sent.

use super::*;
use crate::id::ProviderId;
use crate::sheet::fields::{FieldAnswer, FieldKind, FieldSpec, FieldValue};
use crate::sheet::input::SheetInput;
use crate::sheet::manual::{Protocol, manual_form};
use crate::sheet::progress::{Progress, SignInInput};
use crate::sheet::view::{FieldProblem, ProblemKind, SheetView};
use crate::wire::ProviderHint;

fn provider() -> ProviderId {
    ProviderId::parse("generic-imap").expect("id")
}

fn form() -> Vec<FieldSpec> {
    manual_form(Protocol::Imap, Some("example.org"))
}

fn sheet(stage: Stage) -> Sheet {
    Sheet {
        purpose: Purpose::Add {
            hint: ProviderHint::Any,
            allow: None,
        },
        stage,
        providers: vec![],
    }
}

fn asking(fields: Vec<FieldSpec>, problem: Option<FieldProblem>) -> Stage {
    Stage::Asking {
        provider: provider(),
        fields,
        problem,
    }
}

fn plain(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Plain(text.to_owned()),
    }
}

fn answers(protocol: &str) -> Vec<FieldAnswer> {
    let mut all = vec![plain(FieldKind::Protocol, protocol)];
    match protocol {
        "jmap" => all.push(plain(
            FieldKind::SessionUrl,
            "https://jmap.example.org/session",
        )),
        _ => all.extend([
            plain(FieldKind::Server, "mail.example.org"),
            plain(FieldKind::Security, "starttls"),
            plain(FieldKind::Port, "143"),
            plain(FieldKind::OutgoingServer, "smtp.example.org"),
            plain(FieldKind::OutgoingSecurity, "tls"),
            plain(FieldKind::OutgoingPort, "465"),
        ]),
    }
    all
}

fn submit(stage: Stage, answers: Vec<FieldAnswer>) -> (Stage, Vec<SheetEffect>) {
    let (sheet, effects) = step(sheet(stage), SheetEvent::Input(SheetInput::Submit(answers)));
    (sheet.stage, effects)
}

fn sent(effects: &[SheetEffect]) -> Vec<FieldAnswer> {
    match effects.last() {
        Some(SheetEffect::Feed(SignInInput::Fields(sent))) => sent.clone(),
        other => panic!("expected a Feed, got {other:?}"),
    }
}

#[test]
fn the_server_form_is_asked_when_the_sign_in_asks_for_it() {
    let working = sheet(Stage::Working(provider()));
    let (next, effects) = step(working, SheetEvent::SignIn(Progress::Ask(form())));
    let want = asking(form(), None);
    assert_eq!(next.stage, want);
    assert_eq!(effects, vec![SheetEffect::Show(next.view())]);
    assert!(matches!(next.view(), SheetView::SignIn(_)));
}

#[test]
fn a_whole_imap_form_goes_to_the_sign_in() {
    let (stage, effects) = submit(asking(form(), None), answers("imap"));
    assert_eq!(stage, Stage::Working(provider()));
    assert_eq!(sent(&effects).len(), 7, "the optional login was not typed");
}

#[test]
fn a_switch_to_jmap_sends_the_jmap_form_only() {
    let mut typed = answers("jmap");
    typed.push(plain(FieldKind::Server, "leftover.example.org"));
    let (stage, effects) = submit(asking(form(), None), typed);
    assert_eq!(stage, Stage::Working(provider()));
    let kinds: Vec<_> = sent(&effects).iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [FieldKind::Protocol, FieldKind::SessionUrl]);
}

#[test]
fn a_bad_answer_marks_its_field_on_the_form_the_answers_shape_and_sends_nothing() {
    let mut typed = answers("jmap");
    typed.retain(|a| a.kind != FieldKind::SessionUrl);
    typed.push(plain(FieldKind::SessionUrl, "http://jmap.example.org"));
    let (stage, effects) = submit(asking(form(), None), typed);
    let Stage::Asking {
        fields, problem, ..
    } = &stage
    else {
        panic!("{stage:?}")
    };
    assert_eq!(
        *problem,
        Some(FieldProblem {
            field: FieldKind::SessionUrl,
            problem: ProblemKind::Invalid
        })
    );
    let kinds: Vec<_> = fields.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [
            FieldKind::Protocol,
            FieldKind::SessionUrl,
            FieldKind::Token,
            FieldKind::Username
        ]
    );
    assert!(effects.iter().all(|e| !matches!(e, SheetEffect::Feed(_))));

    let mut typed = answers("imap");
    typed.retain(|a| a.kind != FieldKind::Port);
    typed.push(plain(FieldKind::Port, "70000"));
    let (stage, _) = submit(asking(form(), None), typed);
    let Stage::Asking { problem, .. } = stage else {
        panic!()
    };
    assert_eq!(
        problem,
        Some(FieldProblem {
            field: FieldKind::Port,
            problem: ProblemKind::Invalid
        })
    );
}
