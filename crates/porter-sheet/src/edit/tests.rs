use super::*;
use crate::manual::{Hop, MailServers, Security};

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

fn imap_answers() -> Vec<FieldAnswer> {
    vec![
        plain(FieldKind::Protocol, "imap"),
        plain(FieldKind::Server, "mail.example.test"),
        plain(FieldKind::Security, "tls"),
        plain(FieldKind::Port, "1993"),
        plain(FieldKind::OutgoingServer, "smtp.example.test"),
        plain(FieldKind::OutgoingSecurity, "starttls"),
        plain(FieldKind::OutgoingPort, ""),
    ]
}

#[test]
fn only_a_typed_password_has_servers_to_edit() {
    let all = [
        (AuthKind::Password, None),
        (AuthKind::AppPassword, None),
        (AuthKind::OAuthPkce, Some(ServersFixed::ProviderSignIn)),
        (AuthKind::OAuthMintsKey, Some(ServersFixed::ProviderSignIn)),
        (AuthKind::OAuthPlan, Some(ServersFixed::ProviderSignIn)),
        (AuthKind::LoginFlowV2, Some(ServersFixed::ProviderSignIn)),
        (AuthKind::CloudIdentity, Some(ServersFixed::ProviderSignIn)),
        (AuthKind::None, Some(ServersFixed::NotTyped)),
        (AuthKind::LocalRuntime, Some(ServersFixed::NotTyped)),
        (AuthKind::LocalBridge, Some(ServersFixed::NotTyped)),
        (AuthKind::KeyPair, Some(ServersFixed::NotTyped)),
        (AuthKind::ApiKey, Some(ServersFixed::NotTyped)),
        (AuthKind::AgentLogin, Some(ServersFixed::NotTyped)),
        (AuthKind::OwnProgram, Some(ServersFixed::NotTyped)),
    ];
    for (auth, fixed) in all {
        assert_eq!(servers_fixed(auth), fixed, "{auth:?}");
        assert_eq!(servers_editable(auth), fixed.is_none(), "{auth:?}");
    }
}

#[test]
fn an_edit_reads_the_servers_the_name_and_a_new_password_and_keeps_none_of_them_in_debug() {
    let mut answers = imap_answers();
    answers.push(plain(FieldKind::Name, "Ada Lovelace"));
    answers.push(secret(FieldKind::Password, "hunter2-correct-horse"));
    let edited = EditedServers::from_answers(&answers, Vec::new()).expect("reads");
    assert_eq!(edited.name(), Some("Ada Lovelace"));
    assert!(edited.has_password());
    let Manual::Imap(MailServers {
        incoming: Hop {
            host,
            port,
            security,
        },
        ..
    }) = edited.servers()
    else {
        panic!("imap: {:?}", edited.servers());
    };
    assert_eq!(
        (host.as_str(), *port, *security),
        ("mail.example.test", 1993, Security::Tls)
    );
    let shown = format!("{edited:?}");
    assert!(!shown.contains("hunter2-correct-horse"), "{shown}");
    let (_, name, password) = edited.into_parts();
    assert_eq!(name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(
        password.as_ref().map(SecretText::expose),
        Some("hunter2-correct-horse")
    );
}

#[test]
fn an_app_password_typed_for_an_app_password_field_is_the_new_password() {
    let mut answers = imap_answers();
    answers.push(secret(FieldKind::AppPassword, "app-pw-example"));
    let edited = EditedServers::from_answers(&answers, Vec::new()).expect("reads");
    let (_, name, password) = edited.into_parts();
    assert_eq!(name, None, "no name field was asked");
    assert_eq!(
        password.as_ref().map(SecretText::expose),
        Some("app-pw-example")
    );
}

#[test]
fn no_password_typed_keeps_the_stored_one() {
    let edited = EditedServers::from_answers(&imap_answers(), Vec::new()).expect("reads");
    assert!(!edited.has_password());
    assert_eq!(edited.into_parts().2, None);
}

#[test]
fn a_name_with_a_control_character_or_over_256_characters_does_not_read() {
    for name in ["Ada\nLovelace".to_owned(), "a".repeat(257)] {
        let mut answers = imap_answers();
        answers.push(plain(FieldKind::Name, &name));
        let problem = EditedServers::from_answers(&answers, Vec::new()).expect_err("refused");
        assert_eq!(problem.field, FieldKind::Name);
    }
}

#[test]
fn an_edit_request_shows_no_password_in_debug_and_gives_it_up_once() {
    let current = EditedServers::from_answers(&imap_answers(), Vec::new())
        .expect("reads")
        .into_parts()
        .0;
    let edit = EditServers::new(
        AccountId::parse("mail-ada").expect("id"),
        ProviderId::parse("generic-imap").expect("id"),
        AccountLabel("ada@example.test".into()),
        current,
    )
    .with_name("Ada")
    .keeping(SecretText::new("stored-pw-example"));
    assert_eq!(edit.name(), Some("Ada"));
    let shown = format!("{edit:?}");
    assert!(!shown.contains("stored-pw-example"), "{shown}");
    let (_, _, _, _, _, keep) = edit.into_parts();
    assert_eq!(
        keep.as_ref().map(SecretText::expose),
        Some("stored-pw-example")
    );
}
