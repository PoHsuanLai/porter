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
