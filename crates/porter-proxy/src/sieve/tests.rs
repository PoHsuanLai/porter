use super::*;
use crate::step::{Effect, Input, RelayEnd, Relaying, Side};
use crate::testing::{bytewise, sent, whole};
use porter_core::{CapabilityKind, EndpointUrl, Family, LoginName, SecretText, ServiceEndpoint};

fn password() -> RelayAuth {
    RelayAuth::Password(SecretText::new("hunter2"))
}

fn relay(tls: Tls, auth: RelayAuth) -> SieveRelay {
    SieveRelay::new(RelayPlan {
        endpoint: ServiceEndpoint {
            family: Family::Imap,
            url: EndpointUrl::parse("sieve://mail.example.org").expect("url"),
            tls,
            login: LoginName("ada".into()),
        },
        kind: CapabilityKind::Mail,
        auth,
    })
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

const SECURE: &str = "\"IMPLEMENTATION\" \"Dovecot Pigeonhole\"\r\n\"SIEVE\" \"fileinto\"\r\n\"SASL\" \"PLAIN LOGIN\"\r\n\"VERSION\" \"1.0\"\r\nOK \"ready\"\r\n";

#[test]
fn the_relay_picks_the_mechanism_the_server_and_the_credential_allow() {
    let plain = lines("\"SASL\" \"PLAIN LOGIN\"");
    let oauth = lines("\"SASL\" \"OAUTHBEARER XOAUTH2\"");
    let none = lines("\"SIEVE\" \"fileinto\"");
    let token = RelayAuth::AccessToken(SecretText::new("t"));
    assert_eq!(SieveAuth::choose(&plain, &password()), Ok(SieveAuth::Plain));
    assert_eq!(SieveAuth::choose(&oauth, &token), Ok(SieveAuth::Xoauth2));
    assert_eq!(SieveAuth::choose(&plain, &token), Err(RelayFault::Protocol));
    assert_eq!(
        SieveAuth::choose(&oauth, &password()),
        Err(RelayFault::Protocol)
    );
    assert_eq!(
        SieveAuth::choose(&none, &password()),
        Err(RelayFault::Protocol)
    );
}

#[test]
fn the_app_is_told_the_capabilities_without_sasl_and_starttls() {
    let server = lines(
        "\"IMPLEMENTATION\" \"x\"\n\"STARTTLS\"\n\"SASL\" \"PLAIN\"\n\"SIEVE\" \"fileinto\"\n\"starttls\"",
    );
    assert_eq!(
        String::from_utf8(app_greeting(&server)).expect("text"),
        "\"IMPLEMENTATION\" \"x\"\r\n\"SIEVE\" \"fileinto\"\r\nOK \"porter relay ready\"\r\n"
    );
}

#[test]
fn starttls_then_authenticate_then_relay_a_byte_at_a_time() {
    let relay = relay(Tls::StartTls, password());
    let (relay, effects) = bytewise(
        relay,
        Side::Server,
        b"\"IMPLEMENTATION\" \"x\"\r\n\"STARTTLS\"\r\n\"VERSION\" \"1.0\"\r\nOK \"hi\"\r\n",
    );
    assert_eq!(sent(&effects, Side::Server), "STARTTLS\r\n");
    let (relay, effects) = bytewise(
        relay,
        Side::Server,
        b"OK \"Begin TLS negotiation now.\"\r\n",
    );
    assert_eq!(effects, vec![Effect::StartTls]);
    let (relay, effects) = relay.step(Input::TlsReady);
    assert!(effects.is_empty());
    let (relay, effects) = bytewise(relay, Side::Server, SECURE.as_bytes());
    // base64 of "\0ada\0hunter2"
    assert_eq!(
        sent(&effects, Side::Server),
        "AUTHENTICATE \"PLAIN\" \"AGFkYQBodW50ZXIy\"\r\n"
    );
    assert_eq!(sent(&effects, Side::App), "");
    let (relay, effects) = bytewise(relay, Side::Server, b"OK \"Authenticated\"\r\n");
    let greeting = sent(&effects, Side::App);
    assert!(
        greeting.contains("\"SIEVE\" \"fileinto\"") && !greeting.contains("SASL"),
        "{greeting}"
    );
    assert!(greeting.ends_with("OK \"porter relay ready\"\r\n"));
    let (relay, effects) = whole(relay, Side::App, b"LISTSCRIPTS\r\n");
    assert_eq!(sent(&effects, Side::Server), "LISTSCRIPTS\r\n");
    let (_, effects) = whole(relay, Side::Server, b"OK\r\n");
    assert_eq!(sent(&effects, Side::App), "OK\r\n");
}

#[test]
fn an_error_challenge_is_answered_empty_and_a_no_is_a_refusal() {
    let (relay, _) = whole(
        relay(Tls::Implicit, password()),
        Side::Server,
        SECURE.as_bytes(),
    );
    let (relay, effects) = whole(relay, Side::Server, b"\"eyJzdGF0dXMiOiI0MDEifQ==\"\r\n");
    assert_eq!(sent(&effects, Side::Server), "\"\"\r\n");
    let (_, effects) = whole(relay, Side::Server, b"NO \"Authentication failed\"\r\n");
    assert!(effects.contains(&Effect::Close(RelayEnd::Failed(RelayFault::Refused))));
}

#[test]
fn no_credential_goes_out_in_the_clear() {
    const CASES: &[(&str, Tls, &str)] = &[
        (
            "starttls wanted, not offered",
            Tls::StartTls,
            "\"SASL\" \"PLAIN\"\r\nOK \"hi\"\r\n",
        ),
        (
            "starttls refused",
            Tls::StartTls,
            "\"STARTTLS\"\r\nOK \"hi\"\r\nNO \"nope\"\r\n",
        ),
        (
            "no usable mechanism",
            Tls::Implicit,
            "\"SASL\" \"LOGIN\"\r\nOK \"hi\"\r\n",
        ),
        ("a bye", Tls::Implicit, "BYE \"go away\"\r\n"),
    ];
    for (name, tls, script) in CASES {
        let (_, effects) = whole(relay(*tls, password()), Side::Server, script.as_bytes());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Close(RelayEnd::Failed(_)))),
            "{name}: {effects:?}"
        );
        assert!(
            !sent(&effects, Side::Server).contains("AUTHENTICATE"),
            "{name}"
        );
    }
    // Bytes smuggled after the STARTTLS `OK` are not read as TLS.
    let (relay, _) = whole(
        relay(Tls::StartTls, password()),
        Side::Server,
        b"\"STARTTLS\"\r\nOK \"hi\"\r\n",
    );
    let (_, effects) = whole(relay, Side::Server, b"OK \"go\"\r\n\"injected\"\r\n");
    assert!(effects.contains(&Effect::Close(RelayEnd::Failed(RelayFault::Protocol))));
}
