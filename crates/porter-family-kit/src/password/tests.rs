use super::*;

#[test]
fn basic_encodes_the_login_and_the_password_together() {
    assert_eq!(
        basic("alice", &SecretText::new("secret")),
        "Basic YWxpY2U6c2VjcmV0"
    );
}

#[test]
fn what_was_typed_is_read_by_field_and_a_password_keeps_its_spaces() {
    let answers = vec![
        FieldAnswer {
            kind: FieldKind::Address,
            value: FieldValue::Plain("  ada@example.org ".into()),
        },
        FieldAnswer {
            kind: FieldKind::Password,
            value: FieldValue::Secret(SecretText::new(" pw ")),
        },
        FieldAnswer {
            kind: FieldKind::Server,
            value: FieldValue::Plain("   ".into()),
        },
    ];
    assert_eq!(
        text_of(&answers, FieldKind::Address).as_deref(),
        Some("ada@example.org")
    );
    assert_eq!(
        secret_of(&answers, FieldKind::Password)
            .as_ref()
            .map(SecretText::expose),
        Some(" pw ")
    );
    assert_eq!(
        text_of(&answers, FieldKind::Server),
        None,
        "blank is nothing"
    );
    assert_eq!(text_of(&answers, FieldKind::Username), None);
}

/// ship-5: the password is read from the app password field, or from a password field (a host
/// may answer an app password form with either kind).
#[test]
fn the_typed_password_is_the_app_password_else_the_password() {
    let answer = |kind, text: &str| FieldAnswer {
        kind,
        value: FieldValue::Secret(SecretText::new(text)),
    };
    let read = |answers: &[FieldAnswer]| typed_password(answers).map(|s| s.expose().to_owned());
    assert_eq!(
        read(&[answer(FieldKind::AppPassword, "app")]).as_deref(),
        Some("app")
    );
    assert_eq!(
        read(&[answer(FieldKind::Password, "pw")]).as_deref(),
        Some("pw")
    );
    assert_eq!(
        read(&[
            answer(FieldKind::Password, "pw"),
            answer(FieldKind::AppPassword, "app")
        ])
        .as_deref(),
        Some("app")
    );
    assert_eq!(read(&[answer(FieldKind::AppPassword, "")]), None);
}
