use super::*;

#[test]
fn the_user_id_is_the_last_segment_of_the_principal_and_is_decoded() {
    const CASES: &[(&str, Option<&str>)] = &[
        ("/remote.php/dav/principals/users/ada/", Some("ada")),
        (
            "/remote.php/dav/principals/users/ada%40example.org",
            Some("ada@example.org"),
        ),
        ("/remote.php/dav/principals/users/a%20b/", Some("a b")),
        ("/", None),
        ("/x/%zz/", None),
    ];
    for (path, want) in CASES {
        assert_eq!(user_of(path).as_deref(), *want, "{path}");
    }
}

#[test]
fn a_reply_is_a_refusal_a_success_or_a_fault_by_its_status() {
    let reply = |status| HttpResponse {
        status: porter_http::Status(status),
        headers: vec![],
        body: vec![],
    };
    assert_eq!(judged(reply(401)).err(), Some(SignInFault::Refused));
    assert_eq!(judged(reply(403)).err(), Some(SignInFault::Refused));
    assert!(judged(reply(207)).is_ok());
    assert_eq!(judged(reply(503)).err(), Some(SignInFault::Unreachable));
    assert_eq!(judged(reply(404)).err(), Some(SignInFault::Unreadable));
}
