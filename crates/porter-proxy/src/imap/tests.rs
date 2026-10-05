use super::*;
use porter_core::{
    CapabilityKind, EndpointUrl, Family, LoginName, SecretText, ServiceEndpoint, Tls,
};

fn caps(text: &str) -> Vec<String> {
    text.split(' ').map(str::to_owned).collect()
}

fn password() -> RelayAuth {
    RelayAuth::Password(SecretText::new("hunter2"))
}

fn token() -> RelayAuth {
    RelayAuth::AccessToken(SecretText::new("tok"))
}

#[test]
fn the_relay_picks_the_way_the_server_and_the_credential_allow() {
    const CASES: &[(&str, &str, bool, Result<ImapAuth, RelayFault>)] = &[
        (
            "plain when offered",
            "IMAP4rev1 AUTH=PLAIN AUTH=LOGIN",
            false,
            Ok(ImapAuth::Plain),
        ),
        (
            "login when plain is not offered",
            "IMAP4rev1 IDLE",
            false,
            Ok(ImapAuth::Login),
        ),
        (
            "login disabled, no plain",
            "IMAP4rev1 LOGINDISABLED STARTTLS",
            false,
            Err(RelayFault::Protocol),
        ),
        (
            "a token needs xoauth2",
            "IMAP4rev1 AUTH=XOAUTH2 AUTH=PLAIN",
            true,
            Ok(ImapAuth::Xoauth2),
        ),
        (
            "a token without xoauth2",
            "IMAP4rev1 AUTH=PLAIN",
            true,
            Err(RelayFault::Protocol),
        ),
        (
            "case is not significant",
            "imap4rev1 auth=plain",
            false,
            Ok(ImapAuth::Plain),
        ),
    ];
    for (name, server, use_token, expected) in CASES {
        let auth = if *use_token { token() } else { password() };
        assert_eq!(ImapAuth::choose(&caps(server), &auth), *expected, "{name}");
    }
}

#[test]
fn the_app_is_told_the_capabilities_without_the_ways_to_authenticate() {
    let server =
        caps("IMAP4rev1 STARTTLS AUTH=PLAIN AUTH=XOAUTH2 LOGINDISABLED IDLE UIDPLUS CONDSTORE");
    assert_eq!(
        app_capabilities(&server),
        caps("IMAP4rev1 IDLE UIDPLUS CONDSTORE")
    );
    assert_eq!(
        String::from_utf8(preauth_greeting(&server)).expect("text"),
        "* PREAUTH [CAPABILITY IMAP4rev1 IDLE UIDPLUS CONDSTORE] porter relay ready\r\n"
    );
}

#[test]
fn a_new_relay_waits_for_the_servers_greeting_and_never_shows_its_credential() {
    let plan = RelayPlan {
        endpoint: ServiceEndpoint {
            family: Family::Imap,
            url: EndpointUrl::parse("imaps://imap.example.org").expect("url"),
            tls: Tls::Implicit,
            login: LoginName("ada".into()),
        },
        kind: CapabilityKind::Mail,
        auth: password(),
    };
    let relay = ImapRelay::new(plan);
    assert_eq!(relay.phase, ImapPhase::Greeting);
    assert!(!format!("{relay:?}").contains("hunter2"));
}

mod scripted {
    use super::*;
    use crate::step::{Effect, Input, RelayEnd, Relaying, Side};
    use crate::testing::{bytewise, sent, whole};

    fn relay(tls: Tls, auth: RelayAuth) -> ImapRelay {
        ImapRelay::new(RelayPlan {
            endpoint: ServiceEndpoint {
                family: Family::Imap,
                url: EndpointUrl::parse("imaps://imap.example.org").expect("url"),
                tls,
                login: LoginName("ada".into()),
            },
            kind: CapabilityKind::Mail,
            auth,
        })
    }

    fn closed_with(effects: &[Effect]) -> Option<RelayEnd> {
        effects.iter().find_map(|e| match e {
            Effect::Close(end) => Some(*end),
            _ => None,
        })
    }

    #[test]
    fn a_login_server_is_logged_into_and_the_app_hears_only_preauth() {
        // Fed a byte at a time, so no line is ever whole in one read.
        let (relay, effects) = bytewise(
            relay(Tls::Implicit, password()),
            Side::Server,
            b"* OK [CAPABILITY IMAP4rev1 IDLE] hi\r\n",
        );
        assert_eq!(
            sent(&effects, Side::Server),
            "P4 LOGIN \"ada\" \"hunter2\"\r\n"
        );
        assert_eq!(sent(&effects, Side::App), "");
        let (relay, effects) = bytewise(
            relay,
            Side::Server,
            b"* 1 EXISTS\r\nP4 OK [CAPABILITY IMAP4rev1 IDLE UIDPLUS AUTH=PLAIN] done\r\n",
        );
        assert_eq!(
            sent(&effects, Side::App),
            "* PREAUTH [CAPABILITY IMAP4rev1 IDLE UIDPLUS] porter relay ready\r\n"
        );
        assert_eq!(relay.phase, ImapPhase::Relaying);
        let (relay, effects) = whole(relay, Side::App, b"a SELECT INBOX\r\n");
        assert_eq!(sent(&effects, Side::Server), "a SELECT INBOX\r\n");
        let (_, effects) = whole(relay, Side::Server, b"a OK\r\n");
        assert_eq!(sent(&effects, Side::App), "a OK\r\n");
    }

    #[test]
    fn a_quote_or_backslash_in_the_password_is_escaped_and_a_line_break_refused() {
        let tricky = RelayAuth::Password(SecretText::new("a\"b\\c"));
        let (_, effects) = whole(
            relay(Tls::Implicit, tricky),
            Side::Server,
            b"* OK [CAPABILITY IMAP4rev1] hi\r\n",
        );
        assert_eq!(
            sent(&effects, Side::Server),
            "P4 LOGIN \"ada\" \"a\\\"b\\\\c\"\r\n"
        );
        let breaking = RelayAuth::Password(SecretText::new("a\r\nP4 OK"));
        let (_, effects) = whole(
            relay(Tls::Implicit, breaking),
            Side::Server,
            b"* OK [CAPABILITY IMAP4rev1] hi\r\n",
        );
        assert_eq!(
            closed_with(&effects),
            Some(RelayEnd::Failed(RelayFault::Protocol))
        );
        assert_eq!(sent(&effects, Side::Server), "");
    }

    #[test]
    fn plain_waits_for_the_servers_continuation_and_answers_an_error_challenge_empty() {
        let relay = relay(Tls::Implicit, password());
        let (relay, effects) = whole(
            relay,
            Side::Server,
            b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] hi\r\n",
        );
        assert_eq!(sent(&effects, Side::Server), "P4 AUTHENTICATE PLAIN\r\n");
        let (relay, effects) = whole(relay, Side::Server, b"+ \r\n");
        // base64 of "\0ada\0hunter2"
        assert_eq!(sent(&effects, Side::Server), "AGFkYQBodW50ZXIy\r\n");
        let (relay, effects) = whole(relay, Side::Server, b"+ eyJzdGF0dXMiOiI0MDEifQ==\r\n");
        assert_eq!(sent(&effects, Side::Server), "\r\n");
        let (_, effects) = whole(relay, Side::Server, b"P4 NO [AUTHENTICATIONFAILED] no\r\n");
        assert_eq!(
            closed_with(&effects),
            Some(RelayEnd::Failed(RelayFault::Refused))
        );
    }

    #[test]
    fn a_greeting_without_capabilities_asks_and_a_login_without_them_asks_again() {
        let (relay, effects) = whole(
            relay(Tls::Implicit, password()),
            Side::Server,
            b"* OK hello\r\n",
        );
        assert_eq!(sent(&effects, Side::Server), "P1 CAPABILITY\r\n");
        let (relay, effects) = whole(
            relay,
            Side::Server,
            b"* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\nP1 OK done\r\n",
        );
        assert_eq!(sent(&effects, Side::Server), "P4 AUTHENTICATE PLAIN\r\n");
        let (relay, _) = whole(relay, Side::Server, b"+ \r\n");
        let (relay, effects) = whole(relay, Side::Server, b"P4 OK logged in\r\n");
        assert_eq!(sent(&effects, Side::Server), "P5 CAPABILITY\r\n");
        let (relay, effects) = whole(
            relay,
            Side::Server,
            b"* CAPABILITY IMAP4rev1 IDLE MOVE\r\nP5 OK done\r\n",
        );
        assert_eq!(
            sent(&effects, Side::App),
            "* PREAUTH [CAPABILITY IMAP4rev1 IDLE MOVE] porter relay ready\r\n"
        );
        assert_eq!(relay.phase, ImapPhase::Relaying);
    }

    #[test]
    fn starttls_is_asked_for_before_any_credential_and_capabilities_are_read_again() {
        let relay = relay(Tls::StartTls, password());
        let (relay, effects) = whole(
            relay,
            Side::Server,
            b"* OK [CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED] hi\r\n",
        );
        assert_eq!(sent(&effects, Side::Server), "P2 STARTTLS\r\n");
        let (relay, effects) = whole(relay, Side::Server, b"P2 OK go\r\n");
        assert_eq!(effects, vec![Effect::StartTls]);
        let (relay, effects) = relay.step(Input::TlsReady);
        assert_eq!(sent(&effects, Side::Server), "P3 CAPABILITY\r\n");
        let (_, effects) = whole(
            relay,
            Side::Server,
            b"* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\nP3 OK done\r\n",
        );
        assert_eq!(sent(&effects, Side::Server), "P4 AUTHENTICATE PLAIN\r\n");
    }

    #[test]
    fn no_credential_goes_out_in_the_clear() {
        const CASES: &[(&str, Tls, &[u8])] = &[
            (
                "starttls wanted, not offered",
                Tls::StartTls,
                b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] hi\r\n",
            ),
            (
                "starttls refused",
                Tls::StartTls,
                b"* OK [CAPABILITY IMAP4rev1 STARTTLS] hi\r\nP2 NO nope\r\n",
            ),
            (
                "bytes smuggled after the starttls ok",
                Tls::StartTls,
                b"* OK [CAPABILITY IMAP4rev1 STARTTLS] hi\r\nP2 OK go\r\n* injected\r\n",
            ),
            (
                "login disabled and no plain",
                Tls::Implicit,
                b"* OK [CAPABILITY IMAP4rev1 LOGINDISABLED] hi\r\n",
            ),
            ("a bye", Tls::Implicit, b"* BYE go away\r\n"),
            ("not imap", Tls::Implicit, b"HTTP/1.1 400 Bad Request\r\n"),
        ];
        for (name, tls, script) in CASES {
            let (_, effects) = whole(relay(*tls, password()), Side::Server, script);
            assert!(
                matches!(closed_with(&effects), Some(RelayEnd::Failed(_))),
                "{name}: {effects:?}"
            );
            assert!(!sent(&effects, Side::Server).contains("hunter2"), "{name}");
            assert!(!sent(&effects, Side::Server).contains("LOGIN"), "{name}");
            assert!(
                !sent(&effects, Side::Server).contains("AUTHENTICATE"),
                "{name}"
            );
        }
    }

    #[test]
    fn what_the_app_sends_early_follows_the_greeting_once_authenticated() {
        let relay = relay(Tls::Implicit, password());
        let (relay, effects) = whole(relay, Side::App, b"a NOOP\r\n");
        assert!(effects.is_empty());
        let (relay, effects) = whole(
            relay,
            Side::Server,
            b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] hi\r\n",
        );
        assert!(!sent(&effects, Side::Server).contains("NOOP"));
        let (relay, _) = whole(relay, Side::Server, b"+ \r\n");
        let (_, effects) = whole(relay, Side::Server, b"P4 OK [CAPABILITY IMAP4rev1] in\r\n");
        assert!(sent(&effects, Side::App).starts_with("* PREAUTH"));
        assert_eq!(sent(&effects, Side::Server), "a NOOP\r\n");
    }

    #[test]
    fn closing_before_the_session_ends_the_relay() {
        let (_, effects) = relay(Tls::Implicit, password()).step(Input::Closed(Side::App));
        assert_eq!(closed_with(&effects), Some(RelayEnd::Finished));
        let (_, effects) = relay(Tls::Implicit, password()).step(Input::Closed(Side::Server));
        assert_eq!(
            closed_with(&effects),
            Some(RelayEnd::Failed(RelayFault::Protocol))
        );
    }
}
