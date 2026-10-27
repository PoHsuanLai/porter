//! Editing an account's servers through the machine: the form prefilled from what the account
//! has, checked as the add form is, tried by the sign-in before anything is kept, and a refusal
//! marked on its field with nothing ended.

use super::*;
use crate::auth_kind::AuthKind;
use crate::secret::SecretText;
use crate::sheet::edit::{EditedServers, ServersFixed, servers_editable, servers_fixed};
use crate::sheet::fields::{Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence};
use crate::sheet::input::SheetInput;
use crate::sheet::manual::{Hop, JmapServer, MailServers, Manual, Security};
use crate::sheet::progress::{Progress, SignInFault, SignInInput};
use crate::sheet::view::{FieldProblem, ProblemKind, SheetView};

const SECRET: &str = "hunter2-correct-horse";

fn provider() -> ProviderId {
    ProviderId::parse("generic-imap").expect("id")
}

fn hop(host: &str, port: u16, security: Security) -> Hop {
    Hop {
        host: host.to_owned(),
        port,
        security,
    }
}

fn imap() -> Manual {
    Manual::Imap(MailServers {
        incoming: hop("mail.example.test", 993, Security::Tls),
        outgoing: hop("smtp.example.test", 587, Security::StartTls),
        login: Some("ada".to_owned()),
    })
}

fn jmap() -> Manual {
    Manual::Jmap(JmapServer {
        session: EndpointUrl::parse("https://jmap.example.test/.well-known/jmap").expect("url"),
        login: None,
        token: None,
    })
}

fn purpose(current: Manual) -> Purpose {
    Purpose::EditServers {
        account: AccountId::parse("ada").expect("id"),
        provider: provider(),
        current,
        name: Some("Ada Lovelace".to_owned()),
    }
}

fn opened(current: Manual) -> Sheet {
    Sheet::new(purpose(current), vec![])
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

/// The form sent back as it was prefilled, with `changes` over it.
fn answers(sheet: &Sheet, changes: Vec<FieldAnswer>) -> Vec<FieldAnswer> {
    let Stage::Asking { fields, .. } = &sheet.stage else {
        panic!("asking: {:?}", sheet.stage);
    };
    let mut all: Vec<FieldAnswer> = fields
        .iter()
        .filter(|f| changes.iter().all(|c| c.kind != f.kind))
        .map(|f| match f.entry {
            Entry::Plain => plain(f.kind, f.prefill.as_deref().unwrap_or("")),
            Entry::Secret => secret(f.kind, ""),
        })
        .collect();
    all.extend(changes);
    all
}

fn submit(sheet: Sheet, answers: Vec<FieldAnswer>) -> (Sheet, Vec<SheetEffect>) {
    step(sheet, SheetEvent::Input(SheetInput::Submit(answers)))
}

fn problem_of(stage: &Stage) -> Option<FieldProblem> {
    match stage {
        Stage::Asking { problem, .. } => *problem,
        other => panic!("asking: {other:?}"),
    }
}

fn fed(effects: &[SheetEffect]) -> bool {
    effects
        .iter()
        .any(|e| matches!(e, SheetEffect::Feed(SignInInput::Fields(_))))
}

fn refused(field: FieldKind) -> Option<FieldProblem> {
    Some(FieldProblem {
        field,
        problem: ProblemKind::Refused,
    })
}

fn invalid(field: FieldKind) -> Option<FieldProblem> {
    Some(FieldProblem {
        field,
        problem: ProblemKind::Invalid,
    })
}

/// The sheet after the prefilled form was sent with `changes`, checking.
fn checking(current: Manual, changes: Vec<FieldAnswer>) -> Sheet {
    let sheet = opened(current);
    let typed = answers(&sheet, changes);
    let (sheet, effects) = submit(sheet, typed);
    assert!(matches!(sheet.stage, Stage::Checking { .. }), "{sheet:?}");
    assert!(fed(&effects));
    sheet
}

fn edit_of(sheet: &Sheet) -> EditedServers {
    match &sheet.stage {
        Stage::Checking { edit, .. } => edit.clone(),
        other => panic!("checking: {other:?}"),
    }
}

#[test]
fn the_form_opens_prefilled_from_the_account_with_no_password_and_no_provider_list() {
    let sheet = opened(imap());
    let Stage::Asking {
        fields, problem, ..
    } = &sheet.stage
    else {
        panic!("{:?}", sheet.stage);
    };
    assert_eq!(*problem, None);
    let prefilled: Vec<(FieldKind, Option<&str>)> = fields
        .iter()
        .map(|f| (f.kind, f.prefill.as_deref()))
        .collect();
    assert_eq!(
        prefilled,
        [
            (FieldKind::Name, Some("Ada Lovelace")),
            (FieldKind::Protocol, Some("imap")),
            (FieldKind::Server, Some("mail.example.test")),
            (FieldKind::Security, Some("tls")),
            (FieldKind::Port, Some("993")),
            (FieldKind::OutgoingServer, Some("smtp.example.test")),
            (FieldKind::OutgoingSecurity, Some("starttls")),
            (FieldKind::OutgoingPort, Some("587")),
            (FieldKind::Username, Some("ada")),
            (FieldKind::Password, None),
        ]
    );
    let password = fields.last().expect("password");
    assert_eq!(
        (password.entry, password.presence),
        (Entry::Secret, Presence::Optional)
    );
    assert!(matches!(sheet.view(), SheetView::SignIn(_)));

    let sheet = opened(jmap());
    let Stage::Asking { fields, .. } = &sheet.stage else {
        panic!("{:?}", sheet.stage);
    };
    let prefilled: Vec<(FieldKind, Option<&str>)> = fields
        .iter()
        .map(|f| (f.kind, f.prefill.as_deref()))
        .collect();
    assert_eq!(
        prefilled,
        [
            (FieldKind::Name, Some("Ada Lovelace")),
            (FieldKind::Protocol, Some("jmap")),
            (
                FieldKind::SessionUrl,
                Some("https://jmap.example.test/.well-known/jmap")
            ),
            (FieldKind::Token, None),
            (FieldKind::Username, None),
            (FieldKind::Password, None),
        ]
    );
}

#[test]
fn only_a_password_or_an_app_password_account_may_edit_its_servers() {
    for auth in [AuthKind::Password, AuthKind::AppPassword] {
        assert!(servers_editable(auth), "{auth:?}");
        assert_eq!(servers_fixed(auth), None, "{auth:?}");
    }
    for auth in [
        AuthKind::OAuthPkce,
        AuthKind::OAuthMintsKey,
        AuthKind::OAuthPlan,
        AuthKind::LoginFlowV2,
        AuthKind::CloudIdentity,
    ] {
        assert!(!servers_editable(auth), "{auth:?}");
        assert_eq!(
            servers_fixed(auth),
            Some(ServersFixed::ProviderSignIn),
            "{auth:?}"
        );
    }
    for auth in [
        AuthKind::ApiKey,
        AuthKind::LocalRuntime,
        AuthKind::AgentLogin,
        AuthKind::OwnProgram,
        AuthKind::None,
    ] {
        assert_eq!(
            servers_fixed(auth),
            Some(ServersFixed::NotTyped),
            "{auth:?}"
        );
    }
}

#[test]
fn a_remote_host_is_offered_no_plain_connection_and_one_typed_is_refused_before_anything_is_sent() {
    assert!(!FieldKind::Security.choices().contains(&"plain"));
    assert!(!FieldKind::OutgoingSecurity.choices().contains(&"plain"));
    for (kind, typed) in [
        (FieldKind::Security, "plain"),
        (FieldKind::Security, "none"),
        (FieldKind::OutgoingSecurity, "plain"),
        (FieldKind::OutgoingSecurity, "none"),
    ] {
        let sheet = opened(imap());
        let typed_answers = answers(&sheet, vec![plain(kind, typed)]);
        let (sheet, effects) = submit(sheet, typed_answers);
        assert_eq!(problem_of(&sheet.stage), invalid(kind), "{kind:?} {typed}");
        assert!(!fed(&effects), "{kind:?} {typed}: nothing is tried");
    }
    // This computer may stay plain, as on the add form.
    let local = vec![
        plain(FieldKind::Server, "127.0.0.1"),
        plain(FieldKind::Security, "plain"),
        plain(FieldKind::Port, "31143"),
    ];
    checking(imap(), local);
}

#[test]
fn the_protocol_stays_the_accounts_own() {
    let sheet = opened(imap());
    let typed = answers(
        &sheet,
        vec![
            plain(FieldKind::Protocol, "jmap"),
            plain(FieldKind::SessionUrl, "https://jmap.example.test/session"),
        ],
    );
    let (sheet, effects) = submit(sheet, typed);
    assert_eq!(problem_of(&sheet.stage), invalid(FieldKind::Protocol));
    assert!(!fed(&effects));
}

#[test]
fn the_servers_go_to_the_sign_in_and_only_its_done_ends_the_sheet_with_them() {
    let changes = vec![
        plain(FieldKind::OutgoingServer, "relay.example.test"),
        plain(FieldKind::OutgoingPort, "465"),
        plain(FieldKind::OutgoingSecurity, "tls"),
    ];
    let sheet = opened(imap());
    let typed = answers(&sheet, changes.clone());
    let (sheet, effects) = submit(sheet, typed);
    // The form is not drawn again while the servers are tried, and a second press does nothing.
    assert!(matches!(
        effects.first(),
        Some(SheetEffect::Show(SheetView::Working { .. }))
    ));
    let Some(SheetEffect::Feed(SignInInput::Fields(sent))) = effects.last() else {
        panic!("{effects:?}");
    };
    assert!(sent.iter().any(|a| a.kind == FieldKind::OutgoingServer));
    let again = answers(&opened(imap()), changes);
    let (sheet, effects) = submit(sheet, again);
    assert!(effects.is_empty(), "{effects:?}");

    let edit = edit_of(&sheet);
    let want = Manual::Imap(MailServers {
        incoming: hop("mail.example.test", 993, Security::Tls),
        outgoing: hop("relay.example.test", 465, Security::Tls),
        login: Some("ada".to_owned()),
    });
    assert_eq!(edit.servers, want);
    let (sheet, effects) = step(sheet, SheetEvent::SignIn(Progress::Done));
    assert_eq!(sheet.stage, Stage::Added);
    assert_eq!(
        effects.last(),
        Some(&SheetEffect::Close(SheetEnd::Edited(edit)))
    );
    // Stored-or-not is the host's after that: the machine asks no store of an edit.
    assert!(effects.iter().all(|e| !matches!(e, SheetEffect::Store(_))));
}

#[test]
fn a_failed_check_ends_nothing_and_marks_the_field_it_was_about() {
    let cases = [
        (
            imap(),
            Progress::Failed(SignInFault::Refused),
            refused(FieldKind::Password),
        ),
        (
            imap(),
            Progress::Failed(SignInFault::Unreachable),
            refused(FieldKind::Server),
        ),
        (
            imap(),
            Progress::Refused(FieldProblem {
                field: FieldKind::OutgoingServer,
                problem: ProblemKind::Refused,
            }),
            refused(FieldKind::OutgoingServer),
        ),
        (
            jmap(),
            Progress::Failed(SignInFault::Unreadable),
            refused(FieldKind::SessionUrl),
        ),
    ];
    for (current, progress, want) in cases {
        let changes = vec![secret(FieldKind::Password, SECRET)];
        let sheet = checking(current, changes);
        let (sheet, effects) = step(sheet, SheetEvent::SignIn(progress.clone()));
        assert_eq!(problem_of(&sheet.stage), want, "{progress:?}");
        assert!(
            effects.iter().all(|e| !matches!(e, SheetEffect::Close(_))),
            "{progress:?}: nothing ends"
        );
        // The form is the one sent, its password not carried back.
        let Stage::Asking { fields, .. } = &sheet.stage else {
            panic!()
        };
        let password = fields.iter().find(|f| f.kind == FieldKind::Password);
        assert_eq!(password.map(|f| f.prefill.clone()), Some(None));
        // And it may be sent again.
        let typed = answers(&sheet, vec![secret(FieldKind::Password, SECRET)]);
        let (sheet, effects) = submit(sheet, typed);
        assert!(matches!(sheet.stage, Stage::Checking { .. }));
        assert!(fed(&effects));
    }
    // A fault no field explains ends the attempt, and leaves the form one press away.
    let sheet = checking(imap(), vec![]);
    let (sheet, _) = step(
        sheet,
        SheetEvent::SignIn(Progress::Failed(SignInFault::Forbidden)),
    );
    assert!(matches!(sheet.stage, Stage::Failed { .. }));
    let (sheet, _) = step(sheet, SheetEvent::Input(SheetInput::Back));
    assert!(matches!(sheet.stage, Stage::Asking { problem: None, .. }));
}

#[test]
fn an_empty_password_keeps_the_stored_one_and_a_typed_one_is_handed_over_as_typed() {
    let kept = checking(imap(), vec![secret(FieldKind::Password, "")]);
    assert_eq!(edit_of(&kept).password, None);
    let typed = checking(imap(), vec![secret(FieldKind::Password, " new pw ")]);
    assert_eq!(edit_of(&typed).password, Some(SecretText::new(" new pw ")));
    let (_, effects) = step(typed, SheetEvent::SignIn(Progress::Done));
    let Some(SheetEffect::Close(SheetEnd::Edited(edit))) = effects.last() else {
        panic!("{effects:?}");
    };
    assert_eq!(edit.password, Some(SecretText::new(" new pw ")));
    // Never drawn: no view shows it.
    assert!(effects.iter().all(|e| {
        match e {
            SheetEffect::Show(view) => !serde_json::to_string(view)
                .expect("view")
                .contains("new pw"),
            _ => true,
        }
    }));
}

#[test]
fn the_name_round_trips_and_is_checked() {
    let same = checking(imap(), vec![]);
    assert_eq!(edit_of(&same).name.as_deref(), Some("Ada Lovelace"));
    let renamed = checking(imap(), vec![plain(FieldKind::Name, "  Ada King ")]);
    assert_eq!(edit_of(&renamed).name.as_deref(), Some("Ada King"));
    let cleared = checking(imap(), vec![plain(FieldKind::Name, "")]);
    assert_eq!(edit_of(&cleared).name, None);

    let sheet = opened(imap());
    let typed = answers(&sheet, vec![plain(FieldKind::Name, "Ada\nLovelace")]);
    let (sheet, effects) = submit(sheet, typed);
    assert_eq!(problem_of(&sheet.stage), invalid(FieldKind::Name));
    assert!(!fed(&effects));

    // A name typed survives a refusal: the form asked again shows it.
    let sheet = checking(imap(), vec![plain(FieldKind::Name, "Ada King")]);
    let (sheet, _) = step(
        sheet,
        SheetEvent::SignIn(Progress::Failed(SignInFault::Refused)),
    );
    let Stage::Asking { fields, .. } = &sheet.stage else {
        panic!()
    };
    let name = fields.iter().find(|f| f.kind == FieldKind::Name);
    assert_eq!(name.and_then(|f| f.prefill.as_deref()), Some("Ada King"));
    assert_eq!(fields.first().map(|f| f.kind), Some(FieldKind::Name));
    assert_eq!(fields.last().map(|f| f.kind), Some(FieldKind::Password));

    // The kind crosses the bus by its name.
    let spec = FieldSpec {
        kind: FieldKind::Name,
        entry: Entry::Plain,
        presence: Presence::Optional,
        prefill: Some("Ada Lovelace".to_owned()),
    };
    let json = serde_json::to_string(&spec).expect("json");
    assert!(json.contains(r#""kind":"name""#), "{json}");
    assert_eq!(
        serde_json::from_str::<FieldSpec>(&json).expect("back"),
        spec
    );
}

#[test]
fn an_edit_has_no_step_back_and_closing_it_while_checking_cancels_the_check() {
    let sheet = opened(imap());
    let before = sheet.stage.clone();
    let (sheet, effects) = step(sheet, SheetEvent::Input(SheetInput::Back));
    assert_eq!((sheet.stage.clone(), effects), (before, vec![]));

    let sheet = checking(imap(), vec![]);
    let (_, effects) = step(sheet, SheetEvent::Input(SheetInput::Dismiss));
    assert_eq!(
        effects,
        vec![
            SheetEffect::Feed(SignInInput::Cancel),
            SheetEffect::Close(SheetEnd::Dismissed)
        ]
    );
}
