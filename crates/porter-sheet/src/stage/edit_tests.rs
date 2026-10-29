//! An edit of an account's servers through the machine: opened on the form prefilled from the
//! account, the name asked only when the app asks it, the servers tried (`Checking`) and nothing
//! kept until they pass, a refusal marking the field and starting the sign-in over.

use super::*;
use crate::fields::{FieldAnswer, FieldKind, FieldValue};
use crate::input::SheetInput;
use crate::manual::{Hop, MailServers, Manual, Security};
use crate::progress::{Progress, SignInFault, SignInInput};
use crate::view::{FieldProblem, ProblemKind, SheetView};
use porter_core::SecretText;

fn provider() -> ProviderId {
    ProviderId::parse("generic-imap").expect("id")
}

fn servers() -> Manual {
    let hop = |host: &str, port, security| Hop {
        host: host.to_owned(),
        port,
        security,
    };
    Manual::Imap(MailServers {
        incoming: hop("imap.example.test", 993, Security::Tls),
        outgoing: hop("smtp.example.test", 587, Security::StartTls),
        login: Some("ada.login".to_owned()),
    })
}

fn edit(name: Option<&str>) -> Sheet {
    Sheet::new(
        Purpose::EditServers {
            account: AccountId::parse("mail-ada").expect("id"),
            provider: provider(),
            auth: AuthKind::Password,
            current: servers(),
            name: name.map(str::to_owned),
        },
        Vec::new(),
    )
}

fn plain(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Plain(text.to_owned()),
    }
}

fn secret(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Secret(SecretText::new(text)),
    }
}

fn answers(host: &str) -> Vec<FieldAnswer> {
    vec![
        plain(FieldKind::Protocol, "imap"),
        plain(FieldKind::Server, host),
        plain(FieldKind::Security, "tls"),
        plain(FieldKind::Port, "993"),
        plain(FieldKind::OutgoingServer, "smtp.example.test"),
        plain(FieldKind::OutgoingSecurity, "starttls"),
        plain(FieldKind::OutgoingPort, "587"),
        plain(FieldKind::Username, "ada.login"),
    ]
}

fn fields_of(sheet: &Sheet) -> Vec<(FieldKind, Option<String>)> {
    match &sheet.stage {
        Stage::Asking { fields, .. } | Stage::Checking { fields, .. } => {
            fields.iter().map(|f| (f.kind, f.prefill.clone())).collect()
        }
        other => panic!("not a form: {other:?}"),
    }
}

fn prefill(sheet: &Sheet, kind: FieldKind) -> Option<String> {
    fields_of(sheet)
        .into_iter()
        .find(|(k, _)| *k == kind)
        .and_then(|(_, p)| p)
}

fn submit(sheet: Sheet, answers: Vec<FieldAnswer>) -> (Sheet, Vec<SheetEffect>) {
    step(sheet, SheetEvent::Input(SheetInput::Submit(answers)))
}

#[test]
fn an_edit_opens_on_the_form_prefilled_from_the_account() {
    let sheet = edit(None);
    let kinds: Vec<FieldKind> = fields_of(&sheet).into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        kinds,
        [
            FieldKind::Protocol,
            FieldKind::Server,
            FieldKind::Security,
            FieldKind::Port,
            FieldKind::OutgoingServer,
            FieldKind::OutgoingSecurity,
            FieldKind::OutgoingPort,
            FieldKind::Username,
            FieldKind::Password,
        ],
        "no name unless the app asks for one"
    );
    assert_eq!(
        prefill(&sheet, FieldKind::Server).as_deref(),
        Some("imap.example.test")
    );
    assert_eq!(prefill(&sheet, FieldKind::Port).as_deref(), Some("993"));
    assert_eq!(
        prefill(&sheet, FieldKind::OutgoingSecurity).as_deref(),
        Some("starttls")
    );
    assert_eq!(
        prefill(&sheet, FieldKind::Username).as_deref(),
        Some("ada.login")
    );
    assert_eq!(
        prefill(&sheet, FieldKind::Password),
        None,
        "a secret is never prefilled"
    );
}

#[test]
fn the_name_is_asked_first_and_only_when_the_app_names_one() {
    let sheet = edit(Some("Ada"));
    let first = fields_of(&sheet).into_iter().next();
    assert_eq!(first, Some((FieldKind::Name, Some("Ada".to_owned()))));
    let empty = edit(Some(""));
    assert_eq!(prefill(&empty, FieldKind::Name), None);
    assert!(fields_of(&empty).iter().any(|(k, _)| *k == FieldKind::Name));
}

#[test]
fn an_app_password_account_is_asked_for_an_app_password() {
    let sheet = Sheet::new(
        Purpose::EditServers {
            account: AccountId::parse("mail-ada").expect("id"),
            provider: provider(),
            auth: AuthKind::AppPassword,
            current: servers(),
            name: None,
        },
        Vec::new(),
    );
    let kinds: Vec<FieldKind> = fields_of(&sheet).into_iter().map(|(k, _)| k).collect();
    assert_eq!(kinds.last(), Some(&FieldKind::AppPassword));
    assert!(!kinds.contains(&FieldKind::Password));
}

#[test]
fn sending_the_form_tries_the_servers_and_the_password_goes_only_to_the_sign_in() {
    let mut sent = answers("mail.example.test");
    sent.push(plain(FieldKind::Name, "Ada Lovelace"));
    sent.push(secret(FieldKind::Password, "pw-example-one"));
    let (sheet, effects) = submit(edit(Some("")), sent);
    assert!(
        matches!(sheet.stage, Stage::Checking { .. }),
        "{:?}",
        sheet.stage
    );
    let fed: Vec<&Vec<FieldAnswer>> = effects
        .iter()
        .filter_map(|e| match e {
            SheetEffect::Feed(SignInInput::Fields(a)) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(fed.len(), 1, "{effects:?}");
    let kinds: Vec<FieldKind> = fed[0].iter().map(|a| a.kind).collect();
    assert!(kinds.contains(&FieldKind::Name) && kinds.contains(&FieldKind::Password));
    let shown = format!("{effects:?}{:?}", sheet.view());
    assert!(!shown.contains("pw-example-one"), "{shown}");
    assert!(
        matches!(
            effects.first(),
            Some(SheetEffect::Show(SheetView::Working { .. }))
        ),
        "{effects:?}"
    );
}

#[test]
fn a_password_that_does_not_do_marks_the_password_and_starts_the_sign_in_over() {
    let mut sent = answers("mail.example.test");
    sent.push(secret(FieldKind::Password, "pw-example-one"));
    let (sheet, _) = submit(edit(None), sent);
    let (sheet, effects) = step(
        sheet,
        SheetEvent::SignIn(Progress::Failed(SignInFault::Refused)),
    );
    let Stage::Asking { problem, .. } = &sheet.stage else {
        panic!("asked again: {:?}", sheet.stage);
    };
    assert_eq!(
        *problem,
        Some(FieldProblem {
            field: FieldKind::Password,
            problem: ProblemKind::Refused
        })
    );
    assert_eq!(
        prefill(&sheet, FieldKind::Server).as_deref(),
        Some("mail.example.test")
    );
    assert_eq!(prefill(&sheet, FieldKind::Password), None);
    assert!(
        effects.contains(&SheetEffect::Feed(SignInInput::Start)),
        "{effects:?}"
    );
    assert!(!effects.iter().any(|e| matches!(e, SheetEffect::Close(_))));
}

#[test]
fn a_server_that_does_not_answer_marks_the_server_and_the_form_can_be_sent_again() {
    let (sheet, _) = submit(edit(None), answers("mail.example.test"));
    let (sheet, _) = step(
        sheet,
        SheetEvent::SignIn(Progress::Failed(SignInFault::Unreachable)),
    );
    let Stage::Asking { problem, .. } = &sheet.stage else {
        panic!("asked again: {:?}", sheet.stage);
    };
    assert_eq!(problem.map(|p| p.field), Some(FieldKind::Server));
    // The late answer of the restarted sign-in's first question changes nothing.
    let (sheet, shown) = step(sheet, SheetEvent::SignIn(Progress::Ask(Vec::new())));
    assert!(shown.is_empty());
    let (sheet, effects) = submit(sheet, answers("mail2.example.test"));
    assert!(matches!(sheet.stage, Stage::Checking { .. }));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SheetEffect::Feed(SignInInput::Fields(_))))
    );
}

#[test]
fn a_fault_no_field_is_about_ends_the_edit() {
    let (sheet, _) = submit(edit(None), answers("mail.example.test"));
    let (sheet, _) = step(
        sheet,
        SheetEvent::SignIn(Progress::Failed(SignInFault::Forbidden)),
    );
    assert!(
        matches!(sheet.stage, Stage::Failed { .. }),
        "{:?}",
        sheet.stage
    );
}

#[test]
fn servers_that_pass_ask_the_host_to_store_and_the_sheet_ends_as_added() {
    let (sheet, _) = submit(edit(None), answers("mail.example.test"));
    let (sheet, effects) = step(sheet, SheetEvent::SignIn(Progress::Done));
    assert_eq!(effects, vec![SheetEffect::Store(Vec::new())]);
    let (sheet, effects) = step(sheet, SheetEvent::Stored);
    assert!(matches!(sheet.stage, Stage::Added));
    assert!(
        effects.contains(&SheetEffect::Close(SheetEnd::Added)),
        "{effects:?}"
    );
}

#[test]
fn a_store_that_fails_fails_the_edit() {
    let (sheet, _) = submit(edit(None), answers("mail.example.test"));
    let (sheet, _) = step(sheet, SheetEvent::SignIn(Progress::Done));
    let (sheet, _) = step(sheet, SheetEvent::StoreFailed);
    assert!(matches!(
        sheet.stage,
        Stage::Failed {
            fault: SignInFault::StoreFailed,
            ..
        }
    ));
}

#[test]
fn another_protocol_is_not_an_edit_of_this_account() {
    let mut sent = answers("mail.example.test");
    sent.retain(|a| a.kind != FieldKind::Protocol);
    sent.push(plain(FieldKind::Protocol, "pop3"));
    let (sheet, effects) = submit(edit(None), sent);
    let Stage::Asking { problem, .. } = &sheet.stage else {
        panic!("still asking: {:?}", sheet.stage);
    };
    assert_eq!(problem.map(|p| p.field), Some(FieldKind::Protocol));
    assert!(!effects.iter().any(|e| matches!(e, SheetEffect::Feed(_))));
}

#[test]
fn a_name_that_cannot_be_one_is_marked_before_anything_is_sent() {
    let mut sent = answers("mail.example.test");
    sent.push(plain(FieldKind::Name, &"a".repeat(300)));
    let (sheet, effects) = submit(edit(Some("")), sent);
    let Stage::Asking { problem, .. } = &sheet.stage else {
        panic!("still asking: {:?}", sheet.stage);
    };
    assert_eq!(problem.map(|p| p.field), Some(FieldKind::Name));
    assert!(!effects.iter().any(|e| matches!(e, SheetEffect::Feed(_))));
}

#[test]
fn closing_the_sheet_while_the_servers_are_tried_cancels_the_check() {
    let (sheet, _) = submit(edit(None), answers("mail.example.test"));
    let (_, effects) = step(sheet, SheetEvent::Input(SheetInput::Dismiss));
    assert_eq!(
        effects,
        vec![
            SheetEffect::Feed(SignInInput::Cancel),
            SheetEffect::Close(SheetEnd::Dismissed)
        ]
    );
}
