use super::*;

const ALL: &[&str] = &["imap", "pop3", "jmap"];

fn plain(kind: FieldKind, text: &str) -> FieldAnswer {
    FieldAnswer {
        kind,
        value: FieldValue::Plain(text.to_owned()),
    }
}

fn kinds(fields: &[FieldSpec]) -> Vec<FieldKind> {
    fields.iter().map(|f| f.kind).collect()
}

fn prefill(fields: &[FieldSpec], kind: FieldKind) -> Option<String> {
    fields
        .iter()
        .find(|f| f.kind == kind)
        .and_then(|f| f.prefill.clone())
}

/// A whole IMAP form, every answer at its default.
fn imap_answers() -> Vec<FieldAnswer> {
    vec![
        plain(FieldKind::Protocol, "imap"),
        plain(FieldKind::Server, "mail.example.org"),
        plain(FieldKind::Security, "tls"),
        plain(FieldKind::Port, ""),
        plain(FieldKind::OutgoingServer, "smtp.example.org"),
        plain(FieldKind::OutgoingSecurity, "tls"),
        plain(FieldKind::OutgoingPort, ""),
    ]
}

fn with(mut answers: Vec<FieldAnswer>, kind: FieldKind, text: &str) -> Vec<FieldAnswer> {
    answers.retain(|a| a.kind != kind);
    answers.push(plain(kind, text));
    answers
}

#[test]
fn each_protocol_asks_its_own_fields_in_order() {
    use FieldKind::*;
    let mail = [
        Protocol,
        Server,
        Security,
        Port,
        OutgoingServer,
        OutgoingSecurity,
        OutgoingPort,
        Username,
    ];
    assert_eq!(kinds(&manual_form(super::Protocol::Imap, None)), mail);
    assert_eq!(kinds(&manual_form(super::Protocol::Pop3, None)), mail);
    assert_eq!(
        kinds(&manual_form(super::Protocol::Jmap, None)),
        [Protocol, SessionUrl, Token, Username],
        "JMAP sends through its own server: no outgoing fields; a token may stand for the password"
    );
}

#[test]
fn ports_and_hosts_prefill_from_protocol_and_the_domain() {
    let imap = manual_form(Protocol::Imap, Some("example.org"));
    assert_eq!(
        prefill(&imap, FieldKind::Server).as_deref(),
        Some("imap.example.org")
    );
    assert_eq!(prefill(&imap, FieldKind::Port).as_deref(), Some("993"));
    assert_eq!(
        prefill(&imap, FieldKind::OutgoingServer).as_deref(),
        Some("smtp.example.org")
    );
    assert_eq!(
        prefill(&imap, FieldKind::OutgoingPort).as_deref(),
        Some("465")
    );
    let pop = manual_form(Protocol::Pop3, Some("example.org"));
    assert_eq!(
        prefill(&pop, FieldKind::Server).as_deref(),
        Some("pop.example.org")
    );
    assert_eq!(prefill(&pop, FieldKind::Port).as_deref(), Some("995"));
    let jmap = manual_form(Protocol::Jmap, Some("example.org"));
    assert_eq!(
        prefill(&jmap, FieldKind::SessionUrl).as_deref(),
        Some("https://example.org/.well-known/jmap")
    );
    assert_eq!(
        prefill(&manual_form(Protocol::Imap, None), FieldKind::Server),
        None
    );
}

#[test]
fn ports_follow_protocol_and_security_until_the_person_types_their_own() {
    const CASES: &[(&str, &str, &str, &str, &str)] = &[
        // protocol, security, typed port, want incoming port, name
        ("imap", "tls", "", "993", "tls"),
        ("imap", "starttls", "", "143", "starttls"),
        (
            "imap",
            "starttls",
            "993",
            "143",
            "a usual port follows the security",
        ),
        ("imap", "tls", "1993", "1993", "an unusual port is kept"),
        ("pop3", "tls", "", "995", "pop3 tls"),
        ("pop3", "starttls", "995", "110", "pop3 starttls"),
    ];
    let form = manual_form(Protocol::Imap, Some("example.org"));
    for (protocol, security, typed, want, name) in CASES {
        let answers = with(
            with(
                with(imap_answers(), FieldKind::Protocol, protocol),
                FieldKind::Security,
                security,
            ),
            FieldKind::Port,
            typed,
        );
        let fitted = refit_in(&form, &answers, ALL);
        assert_eq!(
            prefill(&fitted, FieldKind::Port).as_deref(),
            Some(*want),
            "{name}"
        );
    }
    let starttls_out = with(imap_answers(), FieldKind::OutgoingSecurity, "starttls");
    assert_eq!(
        prefill(&refit(&form, &starttls_out), FieldKind::OutgoingPort).as_deref(),
        Some("587")
    );
}

#[test]
fn a_form_is_reshaped_by_its_protocol_and_keeps_what_was_typed() {
    let form = manual_form(Protocol::Imap, Some("example.org"));
    let to_jmap = vec![
        plain(FieldKind::Protocol, "jmap"),
        plain(FieldKind::Username, "ada"),
    ];
    let fitted = refit(&form, &to_jmap);
    assert_eq!(
        kinds(&fitted),
        [
            FieldKind::Protocol,
            FieldKind::SessionUrl,
            FieldKind::Token,
            FieldKind::Username
        ]
    );
    assert_eq!(
        prefill(&fitted, FieldKind::Username).as_deref(),
        Some("ada")
    );
    assert_eq!(
        prefill(&fitted, FieldKind::Protocol).as_deref(),
        Some("jmap")
    );

    // A guessed host follows the protocol; one the person wrote does not.
    let pop = with(
        vec![plain(FieldKind::Protocol, "pop3")],
        FieldKind::Server,
        "imap.example.org",
    );
    assert_eq!(
        prefill(&refit_in(&form, &pop, ALL), FieldKind::Server).as_deref(),
        Some("pop.example.org")
    );
    let own = with(
        vec![plain(FieldKind::Protocol, "pop3")],
        FieldKind::Server,
        "mail.example.org",
    );
    assert_eq!(
        prefill(&refit_in(&form, &own, ALL), FieldKind::Server).as_deref(),
        Some("mail.example.org")
    );
    // Only the protocols a host is offered shape a form.
    assert_eq!(
        prefill(
            &refit_in(&form, &pop, &["imap", "jmap"]),
            FieldKind::Protocol
        )
        .as_deref(),
        Some("imap")
    );
    assert_eq!(
        prefill(&refit(&form, &pop), FieldKind::Protocol).as_deref(),
        Some("pop3")
    );
}

#[test]
fn a_reshaped_form_guesses_from_the_same_domain() {
    let imap = manual_form(Protocol::Imap, Some("example.org"));
    let jmap = refit(&imap, &[plain(FieldKind::Protocol, "jmap")]);
    assert_eq!(
        prefill(&jmap, FieldKind::SessionUrl).as_deref(),
        Some("https://example.org/.well-known/jmap")
    );
    let pop = refit(&jmap, &[plain(FieldKind::Protocol, "pop3")]);
    assert_eq!(
        prefill(&pop, FieldKind::Server).as_deref(),
        Some("pop.example.org")
    );
    assert_eq!(
        prefill(&pop, FieldKind::OutgoingServer).as_deref(),
        Some("smtp.example.org")
    );
    // No domain, no guess.
    let bare = refit(
        &manual_form(Protocol::Imap, None),
        &[plain(FieldKind::Protocol, "jmap")],
    );
    assert_eq!(prefill(&bare, FieldKind::SessionUrl), None);
}

#[test]
fn a_form_that_is_not_the_server_form_is_not_reshaped() {
    let other = vec![FieldSpec {
        kind: FieldKind::Address,
        entry: Entry::Plain,
        presence: Presence::Required,
        prefill: None,
    }];
    assert_eq!(refit(&other, &[plain(FieldKind::Protocol, "jmap")]), other);
    assert_eq!(
        form_problem(&other, &[plain(FieldKind::Address, "a@b.c")]),
        None
    );
}

#[test]
fn each_protocol_parses_to_its_servers() {
    let imap = parse_manual(&imap_answers()).expect("imap");
    let Manual::Imap(m) = imap else {
        panic!("{imap:?}")
    };
    assert_eq!(
        (
            m.incoming.host.as_str(),
            m.incoming.port,
            m.incoming.security
        ),
        ("mail.example.org", 993, Security::Tls)
    );
    assert_eq!((m.outgoing.port, m.outgoing.security), (465, Security::Tls));
    assert_eq!(m.login, None);

    let starttls = with(
        with(imap_answers(), FieldKind::Security, "starttls"),
        FieldKind::OutgoingSecurity,
        "starttls",
    );
    let Manual::Imap(m) = parse_manual(&starttls).expect("starttls") else {
        panic!()
    };
    assert_eq!(
        (m.incoming.port, m.incoming.security),
        (143, Security::StartTls)
    );
    assert_eq!(
        (m.outgoing.port, m.outgoing.security),
        (587, Security::StartTls)
    );

    let pop = with(imap_answers(), FieldKind::Protocol, "pop3");
    let Manual::Pop3(m) = parse_in(&pop, ALL).expect("pop3") else {
        panic!()
    };
    assert_eq!(m.incoming.port, 995);

    let jmap = vec![
        plain(FieldKind::Protocol, "jmap"),
        plain(FieldKind::SessionUrl, "https://jmap.example.org/session"),
        plain(FieldKind::Username, "ada"),
    ];
    let Manual::Jmap(j) = parse_manual(&jmap).expect("jmap") else {
        panic!()
    };
    assert_eq!(j.session.to_string(), "https://jmap.example.org/session");
    assert_eq!(j.login.as_deref(), Some("ada"));
    assert_eq!(
        j.token, None,
        "no token typed: the password is the credential"
    );

    let token = FieldAnswer {
        kind: FieldKind::Token,
        value: FieldValue::Secret(SecretText::new(" api-token ")),
    };
    let with_token = [jmap.clone(), vec![token]].concat();
    let Manual::Jmap(j) = parse_manual(&with_token).expect("jmap token") else {
        panic!()
    };
    assert_eq!(j.token.as_ref().map(SecretText::expose), Some("api-token"));
    assert!(
        !format!("{j:?}").contains("api-token"),
        "a token is redacted"
    );
}

#[test]
fn a_typed_port_and_login_are_used() {
    let answers = with(
        with(imap_answers(), FieldKind::Port, " 1993 "),
        FieldKind::Username,
        "ada.l",
    );
    let manual = parse_manual(&answers).expect("parses");
    let Manual::Imap(m) = &manual else { panic!() };
    assert_eq!(m.incoming.port, 1993);
    assert_eq!(manual.login(), Some("ada.l"));
}

#[test]
fn a_bad_answer_is_a_problem_on_its_field() {
    use FieldKind::*;
    const CASES: &[(FieldKind, &str, FieldKind, ProblemKind)] = &[
        (Port, "0", Port, ProblemKind::Invalid),
        (Port, "65536", Port, ProblemKind::Invalid),
        (Port, "99999999999", Port, ProblemKind::Invalid),
        (Port, "-1", Port, ProblemKind::Invalid),
        (Port, "9x", Port, ProblemKind::Invalid),
        (
            Server,
            "https://mail.example.org",
            Server,
            ProblemKind::Invalid,
        ),
        (
            Server,
            "mail.example.org/path",
            Server,
            ProblemKind::Invalid,
        ),
        (Server, "mail.example.org:993", Server, ProblemKind::Invalid),
        (Server, "ada@mail.example.org", Server, ProblemKind::Invalid),
        (Server, "mail example.org", Server, ProblemKind::Invalid),
        (Server, "", Server, ProblemKind::Missing),
        (
            OutgoingServer,
            "smtp://smtp.example.org",
            OutgoingServer,
            ProblemKind::Invalid,
        ),
        (OutgoingPort, "70000", OutgoingPort, ProblemKind::Invalid),
        (Security, "ssl", Security, ProblemKind::Invalid),
        (Security, "plain", Security, ProblemKind::Invalid),
        (
            OutgoingSecurity,
            "plain",
            OutgoingSecurity,
            ProblemKind::Invalid,
        ),
        (Protocol, "smtp", Protocol, ProblemKind::Invalid),
        (Username, "ada\u{7}", Username, ProblemKind::Invalid),
    ];
    for (kind, text, field, problem) in CASES {
        let answers = with(imap_answers(), *kind, text);
        assert_eq!(
            parse_manual(&answers).err(),
            Some(FieldProblem {
                field: *field,
                problem: *problem
            }),
            "{kind:?} = {text:?}"
        );
    }
}

#[test]
fn a_session_url_must_be_https() {
    const CASES: &[(&str, bool)] = &[
        ("https://jmap.example.org/session", true),
        ("https://jmap.example.org", true),
        ("http://jmap.example.org/session", false),
        ("jmap.example.org/session", false),
        ("ftp://example.org", false),
        ("https://jmap.example.org/a b", false),
        ("https://ada@jmap.example.org", false),
        ("http://127.0.0.1:8080/session", true),
    ];
    for (url, ok) in CASES {
        let answers = vec![
            plain(FieldKind::Protocol, "jmap"),
            plain(FieldKind::SessionUrl, url),
        ];
        assert_eq!(parse_manual(&answers).is_ok(), *ok, "{url}");
    }
    let missing = vec![plain(FieldKind::Protocol, "jmap")];
    assert_eq!(
        parse_manual(&missing).err().map(|p| (p.field, p.problem)),
        Some((FieldKind::SessionUrl, ProblemKind::Missing))
    );
}

#[test]
fn plain_is_for_this_computer_only() {
    let local = with(
        with(imap_answers(), FieldKind::Server, "127.0.0.1"),
        FieldKind::Security,
        "plain",
    );
    let Manual::Imap(m) = parse_manual(&local).expect("loopback may be plain") else {
        panic!()
    };
    assert_eq!(
        (m.incoming.security, m.incoming.port),
        (Security::Plain, 143)
    );
}

#[test]
fn the_form_problem_is_the_first_missing_field_then_the_first_invalid_answer() {
    let form = manual_form(Protocol::Imap, Some("example.org"));
    assert_eq!(form_problem(&form, &imap_answers()), None);
    let empty_host = with(imap_answers(), FieldKind::Server, "  ");
    assert_eq!(
        form_problem(&form, &empty_host),
        Some(FieldProblem {
            field: FieldKind::Server,
            problem: ProblemKind::Missing
        })
    );
    let bad = with(
        with(imap_answers(), FieldKind::Port, "0"),
        FieldKind::OutgoingPort,
        "x",
    );
    assert_eq!(
        form_problem(&form, &bad),
        Some(FieldProblem {
            field: FieldKind::Port,
            problem: ProblemKind::Invalid
        }),
        "the first in form order"
    );
}

#[test]
fn choices_are_the_values_a_host_offers() {
    assert_eq!(FieldKind::Protocol.choices(), ["imap", "pop3", "jmap"]);
    assert_eq!(FieldKind::Security.choices(), ["tls", "starttls"]);
    assert_eq!(FieldKind::OutgoingSecurity.choices(), ["tls", "starttls"]);
    assert!(FieldKind::Server.choices().is_empty());
    for kind in [FieldKind::Protocol, FieldKind::Security] {
        for choice in kind.choices() {
            let answers = vec![plain(kind, choice)];
            assert!(
                text(&answers, kind).is_some(),
                "{choice} is writable as an answer"
            );
        }
    }
}

#[test]
fn an_old_sign_in_view_still_reads() {
    use crate::sheet::view::SignInView;
    // The JSON of a view written before the server form: only the first six kinds, a problem
    // with the two kinds it had.
    let old = r#"{"provider":"generic-imap","fields":[
        {"kind":"server","entry":"plain","presence":"required","prefill":null},
        {"kind":"password","entry":"secret","presence":"required","prefill":null}],
        "problem":{"field":"server","problem":"refused"}}"#;
    let view: SignInView = serde_json::from_str(old).expect("an old view reads");
    assert_eq!(
        kinds(&view.fields),
        [FieldKind::Server, FieldKind::Password]
    );
    assert_eq!(view.problem.map(|p| p.problem), Some(ProblemKind::Refused));
    // And a new view reads back.
    assert_eq!(view.row, None);
    let new = SignInView {
        provider: view.provider.clone(),
        row: None,
        fields: manual_form(Protocol::Jmap, None),
        problem: Some(FieldProblem {
            field: FieldKind::SessionUrl,
            problem: ProblemKind::Invalid,
        }),
    };
    let json = serde_json::to_string(&new).expect("json");
    assert!(json.contains(r#""kind":"session_url""#) && json.contains(r#""problem":"invalid""#));
    assert_eq!(
        serde_json::from_str::<SignInView>(&json).expect("back"),
        new
    );
}
