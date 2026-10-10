use super::*;
use porter_core::SecretText;

const SERVER: &[u8] = b"250-mail.example.org Hello\r\n250-PIPELINING\r\n250-SIZE 35882577\r\n\
250-STARTTLS\r\n250-AUTH PLAIN LOGIN XOAUTH2\r\n250 8BITMIME\r\n";

fn reply() -> EhloReply {
    EhloReply::parse(SERVER).expect("parses")
}

#[test]
fn an_ehlo_reply_parses_to_its_domain_and_extensions() {
    let reply = reply();
    assert_eq!(reply.domain, "mail.example.org Hello");
    assert_eq!(
        reply.extensions,
        [
            "PIPELINING",
            "SIZE 35882577",
            "STARTTLS",
            "AUTH PLAIN LOGIN XOAUTH2",
            "8BITMIME"
        ]
    );
    assert!(reply.offers("starttls") && reply.offers("SIZE") && !reply.offers("CHUNKING"));
}

#[test]
fn a_reply_that_is_not_a_complete_250_is_refused() {
    const CASES: &[(&str, &[u8])] = &[
        ("empty", b""),
        ("another code", b"554 no service\r\n"),
        ("a continuation that never ends", b"250-a\r\n250-b\r\n"),
        ("not text", b"\xff\xfe\r\n"),
    ];
    for (name, bytes) in CASES {
        assert_eq!(EhloReply::parse(bytes), Err(RelayFault::Protocol), "{name}");
    }
}

#[test]
fn the_app_is_offered_everything_but_starttls_and_auth() {
    assert_eq!(
        String::from_utf8(reply().offered_to_app()).expect("text"),
        "250-mail.example.org Hello\r\n250-PIPELINING\r\n250-SIZE 35882577\r\n250 8BITMIME\r\n"
    );
    let only_auth = EhloReply::parse(b"250-host\r\n250 AUTH PLAIN\r\n").expect("parses");
    assert_eq!(
        String::from_utf8(only_auth.offered_to_app()).expect("text"),
        "250 host\r\n",
        "a reply left with one line ends there"
    );
    let legacy =
        EhloReply::parse(b"250-host\r\n250-AUTH=PLAIN LOGIN\r\n250 SIZE 1\r\n").expect("parses");
    assert!(
        !String::from_utf8(legacy.offered_to_app())
            .expect("text")
            .contains("AUTH")
    );
    assert_eq!(app_greeting(), b"220 porter ESMTP ready\r\n");
}

#[test]
fn the_relay_picks_the_mechanism_the_server_and_the_credential_allow() {
    let password = RelayAuth::Password(SecretText::new("pw"));
    let token = RelayAuth::AccessToken(SecretText::new("tok"));
    let plain_only = EhloReply::parse(b"250-host\r\n250 AUTH PLAIN LOGIN\r\n").expect("parses");
    let none = EhloReply::parse(b"250 host\r\n").expect("parses");
    assert_eq!(SmtpAuth::choose(&reply(), &password), Ok(SmtpAuth::Plain));
    assert_eq!(SmtpAuth::choose(&reply(), &token), Ok(SmtpAuth::Xoauth2));
    assert_eq!(
        SmtpAuth::choose(&plain_only, &token),
        Err(RelayFault::Protocol)
    );
    assert_eq!(
        SmtpAuth::choose(&none, &password),
        Err(RelayFault::Protocol)
    );
}

mod scripted {
    use super::*;
    use crate::step::{Effect, Input, RelayEnd, Relaying, Side};
    use crate::testing::{bytewise, sent, whole};
    use porter_core::{
        CapabilityKind, EndpointUrl, Family, LoginName, RelayPlan, ServiceEndpoint, Tls,
    };

    fn relay(tls: Tls) -> SmtpRelay {
        SmtpRelay::new(RelayPlan::new(
            ServiceEndpoint {
                family: Family::Smtp,
                url: EndpointUrl::parse("smtps://smtp.example.org").expect("url"),
                tls,
                login: LoginName("ada".into()),
            },
            CapabilityKind::Mail,
            RelayAuth::Password(SecretText::new("hunter2")),
        ))
    }

    /// A relay that has authenticated and been greeted, waiting for the app's `EHLO`.
    fn authenticated() -> SmtpRelay {
        let (relay, _) = whole(relay(Tls::Implicit), Side::Server, b"220 hi\r\n");
        let (relay, _) = whole(relay, Side::Server, SERVER_PLAIN);
        let (relay, effects) = whole(relay, Side::Server, b"235 ok\r\n");
        assert_eq!(sent(&effects, Side::App), "220 porter ESMTP ready\r\n");
        relay
    }

    const SERVER_PLAIN: &[u8] = b"250-mail.example.org\r\n250-CHUNKING\r\n250 AUTH PLAIN\r\n";

    fn ready() -> SmtpRelay {
        let (relay, effects) = whole(authenticated(), Side::App, b"EHLO app\r\n");
        assert_eq!(
            sent(&effects, Side::App),
            "250-mail.example.org\r\n250 CHUNKING\r\n"
        );
        relay
    }

    #[test]
    fn the_handshake_runs_a_byte_at_a_time_and_the_app_is_greeted_after_auth() {
        let (relay, effects) = bytewise(
            relay(Tls::Implicit),
            Side::Server,
            b"220-hi\r\n220 there\r\n",
        );
        assert_eq!(sent(&effects, Side::Server), "EHLO localhost\r\n");
        let (relay, effects) = bytewise(relay, Side::Server, SERVER_PLAIN);
        // base64 of "\0ada\0hunter2"
        assert_eq!(
            sent(&effects, Side::Server),
            "AUTH PLAIN AGFkYQBodW50ZXIy\r\n"
        );
        assert_eq!(sent(&effects, Side::App), "");
        let (relay, effects) = bytewise(relay, Side::Server, b"235 2.7.0 ok\r\n");
        assert_eq!(sent(&effects, Side::App), "220 porter ESMTP ready\r\n");
        assert!(matches!(relay.phase, SmtpPhase::AppEhlo(_)));
    }

    #[test]
    fn starttls_comes_before_auth_and_ehlo_is_repeated_after_it() {
        let (relay, effects) = whole(relay(Tls::StartTls), Side::Server, b"220 hi\r\n");
        assert_eq!(sent(&effects, Side::Server), "EHLO localhost\r\n");
        let (relay, effects) = whole(
            relay,
            Side::Server,
            b"250-host\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n",
        );
        assert_eq!(sent(&effects, Side::Server), "STARTTLS\r\n");
        let (relay, effects) = whole(relay, Side::Server, b"220 go\r\n");
        assert_eq!(effects, vec![Effect::StartTls]);
        let (relay, effects) = relay.step(Input::TlsReady);
        assert_eq!(sent(&effects, Side::Server), "EHLO localhost\r\n");
        let (_, effects) = whole(relay, Side::Server, b"250-host\r\n250 AUTH PLAIN\r\n");
        assert!(sent(&effects, Side::Server).starts_with("AUTH PLAIN "));
    }

    #[test]
    fn no_credential_goes_out_in_the_clear_or_after_a_refusal() {
        const CASES: &[(&str, Tls, &[&[u8]], RelayFault)] = &[
            (
                "starttls wanted, not offered",
                Tls::StartTls,
                &[b"220 hi\r\n", b"250-h\r\n250 AUTH PLAIN\r\n"],
                RelayFault::Protocol,
            ),
            (
                "bytes after the 220 that starts TLS",
                Tls::StartTls,
                &[
                    b"220 hi\r\n",
                    b"250-h\r\n250 STARTTLS\r\n",
                    b"220 go\r\n220 injected\r\n",
                ],
                RelayFault::Protocol,
            ),
            (
                "no usable mechanism",
                Tls::Implicit,
                &[b"220 hi\r\n", b"250-h\r\n250 AUTH LOGIN\r\n"],
                RelayFault::Protocol,
            ),
            (
                "a refusal",
                Tls::Implicit,
                &[b"220 hi\r\n", b"250-h\r\n250 AUTH PLAIN\r\n", b"535 no\r\n"],
                RelayFault::Refused,
            ),
            (
                "not smtp",
                Tls::Implicit,
                &[b"HTTP/1.1 400 Bad Request\r\n"],
                RelayFault::Protocol,
            ),
        ];
        for (name, tls, script, fault) in CASES {
            let mut relay = relay(*tls);
            let mut all = Vec::new();
            for chunk in *script {
                let (next, effects) = whole(relay, Side::Server, chunk);
                relay = next;
                all.extend(effects);
            }
            assert!(
                all.contains(&Effect::Close(RelayEnd::Failed(*fault))),
                "{name}: {all:?}"
            );
            assert_eq!(sent(&all, Side::App), "", "{name}");
        }
    }

    #[test]
    fn an_app_auth_or_starttls_is_answered_here_and_never_forwarded() {
        let relay = ready();
        let (relay, effects) = whole(relay, Side::App, b"AUTH PLAIN AGZvbwBiYXI=\r\nSTARTTLS\r\n");
        assert_eq!(sent(&effects, Side::Server), "");
        let answers = sent(&effects, Side::App);
        assert!(
            answers.starts_with("503 ") && answers.matches("503 ").count() == 2,
            "{answers}"
        );
        let (relay, effects) = whole(relay, Side::App, b"EHLO again\r\n");
        assert_eq!(sent(&effects, Side::Server), "");
        assert!(sent(&effects, Side::App).starts_with("250-mail.example.org"));
        let (_, effects) = whole(relay, Side::App, b"MAIL FROM:<a@b>\r\n");
        assert_eq!(sent(&effects, Side::Server), "MAIL FROM:<a@b>\r\n");
    }

    #[test]
    fn a_body_is_passed_unread_to_its_end_wherever_the_reads_split() {
        const BODY: &[u8] = b"Subject: x\r\n\r\nAUTH PLAIN nope\r\n.\r\n";
        for cut in 0..BODY.len() {
            let relay = ready();
            let (relay, _) = whole(relay, Side::App, b"DATA\r\n");
            let (relay, effects) = whole(relay, Side::Server, b"354 go\r\n");
            assert_eq!(sent(&effects, Side::App), "354 go\r\n");
            assert_eq!(relay.phase, SmtpPhase::Data);
            let (left, right) = BODY.split_at(cut);
            let (relay, mut effects) = match left.is_empty() {
                true => (relay, Vec::new()),
                false => whole(relay, Side::App, left),
            };
            let (relay, more) = match right.is_empty() {
                true => (relay, Vec::new()),
                false => whole(relay, Side::App, right),
            };
            effects.extend(more);
            assert_eq!(sent(&effects, Side::Server).as_bytes(), BODY, "cut {cut}");
            assert!(
                sent(&effects, Side::App).is_empty(),
                "cut {cut}: no 503 for body text"
            );
            assert_eq!(relay.phase, SmtpPhase::Relaying, "cut {cut}");
        }
    }

    #[test]
    fn a_command_after_the_body_in_the_same_read_is_read_as_a_command() {
        let relay = ready();
        let (relay, _) = whole(relay, Side::App, b"DATA\r\n");
        let (relay, _) = whole(relay, Side::Server, b"354 go\r\n");
        let (_, effects) = whole(relay, Side::App, b"hi\r\n.\r\nSTARTTLS\r\n");
        assert_eq!(sent(&effects, Side::Server), "hi\r\n.\r\n");
        assert!(sent(&effects, Side::App).starts_with("503"));
    }

    #[test]
    fn a_go_ahead_split_across_reads_still_starts_the_body() {
        let relay = ready();
        let (relay, _) = whole(relay, Side::App, b"DATA\r\n");
        let (relay, _) = bytewise(relay, Side::Server, b"354 go\r\n");
        assert_eq!(relay.phase, SmtpPhase::Data);
        // Another reply that is not 354 does not.
        let relay = ready();
        let (relay, _) = bytewise(relay, Side::Server, b"250 ok\r\n503 no 354 here\r\n");
        assert_eq!(relay.phase, SmtpPhase::Relaying);
    }

    #[test]
    fn a_bdat_chunk_is_passed_by_length_and_may_hold_command_lookalikes() {
        let relay = ready();
        let (relay, effects) = whole(
            relay,
            Side::App,
            b"BDAT 11 LAST\r\nAUTH x\r\nabcSTARTTLS\r\n",
        );
        assert_eq!(
            sent(&effects, Side::Server),
            "BDAT 11 LAST\r\nAUTH x\r\nabc"
        );
        assert_eq!(sent(&effects, Side::App).matches("503").count(), 1);
        assert_eq!(relay.phase, SmtpPhase::Relaying);

        let relay = ready();
        let (relay, effects) = bytewise(relay, Side::App, b"BDAT 4\r\nAU");
        assert_eq!(relay.phase, SmtpPhase::Chunk { remaining: 2 });
        assert_eq!(sent(&effects, Side::Server), "BDAT 4\r\nAU");
    }

    #[test]
    fn before_ehlo_only_ehlo_helo_and_quit_are_answered() {
        let (relay, effects) = whole(authenticated(), Side::App, b"MAIL FROM:<a@b>\r\n");
        assert!(sent(&effects, Side::App).starts_with("503"));
        assert_eq!(sent(&effects, Side::Server), "");
        let (_, effects) = whole(relay, Side::App, b"QUIT\r\n");
        assert!(sent(&effects, Side::App).starts_with("221"));
        assert!(effects.contains(&Effect::Close(RelayEnd::Finished)));
    }
}
