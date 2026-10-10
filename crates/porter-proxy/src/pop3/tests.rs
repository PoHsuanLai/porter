use super::*;
use crate::step::{Effect, Input, RelayEnd, Relaying, Side};
use crate::testing::{bytewise, sent, whole};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::{
    CapabilityKind, EndpointUrl, Family, LoginName, RelayPlan, SecretText, ServiceEndpoint, Tls,
};

fn caps(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}

fn password() -> RelayAuth {
    RelayAuth::Password(SecretText::new("hunter2"))
}

fn token() -> RelayAuth {
    RelayAuth::AccessToken(SecretText::new("tok-1"))
}

#[test]
fn the_relay_picks_the_way_the_server_and_the_credential_allow() {
    type Case = (
        &'static str,
        &'static [&'static str],
        bool,
        Result<Pop3Auth, RelayFault>,
    );
    const CASES: &[Case] = &[
        ("user and pass", &["USER", "TOP"], false, Ok(Pop3Auth::User)),
        ("no capa at all", &[], false, Ok(Pop3Auth::User)),
        (
            "sasl plain and no user",
            &["SASL PLAIN"],
            false,
            Ok(Pop3Auth::Plain),
        ),
        (
            "user wins over sasl plain",
            &["USER", "SASL PLAIN"],
            false,
            Ok(Pop3Auth::User),
        ),
        (
            "a token needs xoauth2",
            &["SASL PLAIN XOAUTH2"],
            true,
            Ok(Pop3Auth::Xoauth2),
        ),
        (
            "a token with only plain",
            &["USER", "SASL PLAIN"],
            true,
            Err(RelayFault::Protocol),
        ),
    ];
    for (name, lines, as_token, want) in CASES {
        let auth = match as_token {
            true => token(),
            false => password(),
        };
        assert_eq!(Pop3Auth::choose(&caps(lines), &auth), *want, "{name}");
    }
    assert_eq!(
        Pop3Auth::choose(&[], &RelayAuth::Anonymous),
        Err(RelayFault::Protocol)
    );
    assert_eq!(app_greeting(), b"+OK porter relay ready\r\n");
}

fn relay(tls: Tls, auth: RelayAuth) -> Pop3Relay {
    let scheme = match tls {
        Tls::Implicit => "pop3s",
        _ => "pop3",
    };
    Pop3Relay::new(RelayPlan::new(
        ServiceEndpoint {
            family: Family::Pop3,
            url: EndpointUrl::parse(&format!("{scheme}://pop.example.org")).expect("url"),
            tls,
            login: LoginName("ada".into()),
        },
        CapabilityKind::Mail,
        auth,
    ))
}

const CAPA: &[u8] = b"+OK capabilities follow\r\nUSER\r\nTOP\r\nSASL PLAIN XOAUTH2\r\n.\r\n";

/// A relay that has read the greeting and the capabilities and sent its `USER`.
fn at_user() -> Pop3Relay {
    let (relay, effects) = whole(
        relay(Tls::Implicit, password()),
        Side::Server,
        b"+OK hi\r\n",
    );
    assert_eq!(sent(&effects, Side::Server), "CAPA\r\n");
    let (relay, effects) = whole(relay, Side::Server, CAPA);
    assert_eq!(sent(&effects, Side::Server), "USER ada\r\n");
    relay
}

fn ready() -> Pop3Relay {
    let (relay, effects) = whole(at_user(), Side::Server, b"+OK\r\n");
    assert_eq!(sent(&effects, Side::Server), "PASS hunter2\r\n");
    let (relay, effects) = whole(relay, Side::Server, b"+OK maildrop locked\r\n");
    assert_eq!(sent(&effects, Side::App), "+OK porter relay ready\r\n");
    assert_eq!(relay.phase, Pop3Phase::Relaying);
    relay
}

#[test]
fn a_password_goes_by_user_and_pass_then_the_app_is_greeted() {
    ready();
}

#[test]
fn a_split_read_changes_nothing() {
    let (relay, effects) = bytewise(
        relay(Tls::Implicit, password()),
        Side::Server,
        b"+OK hi\r\n+OK capabilities follow\r\nUSER\r\n.\r\n+OK\r\n+OK locked\r\n",
    );
    assert_eq!(relay.phase, Pop3Phase::Relaying);
    assert_eq!(
        sent(&effects, Side::Server),
        "CAPA\r\nUSER ada\r\nPASS hunter2\r\n"
    );
    assert_eq!(sent(&effects, Side::App), "+OK porter relay ready\r\n");
}

#[test]
fn a_server_without_capa_is_still_signed_in_to_by_user_and_pass() {
    let (relay, _) = whole(
        relay(Tls::Implicit, password()),
        Side::Server,
        b"+OK hi\r\n",
    );
    let (_, effects) = whole(relay, Side::Server, b"-ERR unknown command\r\n");
    assert_eq!(sent(&effects, Side::Server), "USER ada\r\n");
}

#[test]
fn starttls_upgrades_before_any_credential_is_sent() {
    let (relay, _) = whole(
        relay(Tls::StartTls, password()),
        Side::Server,
        b"+OK hi\r\n",
    );
    let (relay, effects) = whole(relay, Side::Server, b"+OK\r\nUSER\r\nSTLS\r\n.\r\n");
    assert_eq!(sent(&effects, Side::Server), "STLS\r\n");
    let (relay, effects) = whole(relay, Side::Server, b"+OK begin TLS\r\n");
    assert_eq!(effects, vec![Effect::StartTls]);
    let (relay, effects) = relay.step(Input::TlsReady);
    assert_eq!(sent(&effects, Side::Server), "CAPA\r\n");
    let (relay, effects) = whole(relay, Side::Server, b"+OK\r\nUSER\r\n.\r\n");
    assert_eq!(sent(&effects, Side::Server), "USER ada\r\n");
    assert!(!sent(&effects, Side::Server).contains("hunter2"));
    assert_eq!(relay.phase, Pop3Phase::User);
}

#[test]
fn a_server_that_offers_no_stls_is_never_sent_the_password() {
    let (relay, _) = whole(
        relay(Tls::StartTls, password()),
        Side::Server,
        b"+OK hi\r\n",
    );
    let (_, effects) = whole(relay, Side::Server, b"+OK\r\nUSER\r\n.\r\n");
    assert_eq!(
        effects,
        vec![Effect::Close(RelayEnd::Failed(RelayFault::Protocol))]
    );
}

#[test]
fn bytes_after_the_stls_answer_are_not_read_as_tls() {
    let (relay, _) = whole(
        relay(Tls::StartTls, password()),
        Side::Server,
        b"+OK hi\r\n",
    );
    let (relay, _) = whole(relay, Side::Server, b"+OK\r\nSTLS\r\n.\r\n");
    let (_, effects) = whole(relay, Side::Server, b"+OK go\r\ninjected");
    assert_eq!(
        effects,
        vec![Effect::Close(RelayEnd::Failed(RelayFault::Protocol))]
    );
}

#[test]
fn a_refusal_is_the_credentials_and_a_locked_maildrop_is_not() {
    const CASES: &[(&str, &str, &str, RelayFault)] = &[
        ("bad user", "-ERR no such user\r\n", "", RelayFault::Refused),
        (
            "bad password",
            "+OK\r\n",
            "-ERR bad password\r\n",
            RelayFault::Refused,
        ),
        (
            "locked",
            "+OK\r\n",
            "-ERR [IN-USE] maildrop busy\r\n",
            RelayFault::Unreachable,
        ),
        (
            "delayed",
            "+OK\r\n",
            "-ERR [LOGIN-DELAY] wait\r\n",
            RelayFault::Unreachable,
        ),
    ];
    for (name, first, second, fault) in CASES {
        let (relay, effects) = whole(at_user(), Side::Server, first.as_bytes());
        let effects = match second.is_empty() {
            true => effects,
            false => whole(relay, Side::Server, second.as_bytes()).1,
        };
        assert_eq!(
            effects.last(),
            Some(&Effect::Close(RelayEnd::Failed(*fault))),
            "{name}"
        );
    }
}

#[test]
fn a_token_goes_by_xoauth2_and_an_error_challenge_is_answered() {
    let (relay, _) = whole(relay(Tls::Implicit, token()), Side::Server, b"+OK hi\r\n");
    let (relay, effects) = whole(relay, Side::Server, CAPA);
    let line = sent(&effects, Side::Server);
    let encoded = line.strip_prefix("AUTH XOAUTH2 ").expect("xoauth2");
    let decoded = STANDARD.decode(encoded.trim()).expect("base64");
    assert_eq!(decoded, b"user=ada\x01auth=Bearer tok-1\x01\x01");
    let (relay, effects) = whole(relay, Side::Server, b"+ eyJzdGF0dXMiOiI0MDEifQ==\r\n");
    assert_eq!(sent(&effects, Side::Server), "\r\n");
    let (_, effects) = whole(relay, Side::Server, b"-ERR invalid token\r\n");
    assert_eq!(
        effects,
        vec![Effect::Close(RelayEnd::Failed(RelayFault::Refused))]
    );
}

#[test]
fn sasl_plain_is_used_when_the_server_offers_no_user() {
    let (relay, _) = whole(
        relay(Tls::Implicit, password()),
        Side::Server,
        b"+OK hi\r\n",
    );
    let (relay, effects) = whole(relay, Side::Server, b"+OK\r\nSASL PLAIN\r\n.\r\n");
    let line = sent(&effects, Side::Server);
    let encoded = line.strip_prefix("AUTH PLAIN ").expect("plain");
    assert_eq!(
        STANDARD.decode(encoded.trim()).expect("base64"),
        b"\0ada\0hunter2"
    );
    let (_, effects) = whole(relay, Side::Server, b"+OK welcome\r\n");
    assert_eq!(sent(&effects, Side::App), "+OK porter relay ready\r\n");
}

#[test]
fn the_apps_login_is_answered_here_and_the_rest_passes_on() {
    let (relay, effects) = whole(
        ready(),
        Side::App,
        b"CAPA\r\nUSER someone\r\nPASS whatever\r\nAUTH PLAIN x\r\nSTLS\r\nSTAT\r\n",
    );
    let to_app = sent(&effects, Side::App);
    assert_eq!(
        to_app,
        "+OK already authenticated\r\n+OK already authenticated\r\n-ERR already authenticated\r\n-ERR TLS is already active\r\n"
    );
    assert_eq!(sent(&effects, Side::Server), "CAPA\r\nSTAT\r\n");
    assert!(!sent(&effects, Side::Server).contains("whatever"));
    // The server's reply, a message among it, reaches the app as it is.
    let (_, effects) = whole(
        relay,
        Side::Server,
        b"+OK 2 320\r\n+OK message follows\r\nUSER looks like a command\r\n.\r\n",
    );
    assert_eq!(
        sent(&effects, Side::App),
        "+OK 2 320\r\n+OK message follows\r\nUSER looks like a command\r\n.\r\n"
    );
}

#[test]
fn what_the_app_sent_before_the_greeting_is_handled_after_it() {
    let (relay, effects) = whole(at_user(), Side::App, b"USER ada\r\nPASS pw\r\nSTAT\r\n");
    assert!(effects.is_empty(), "held until the relay is ready");
    let (relay, _) = whole(relay, Side::Server, b"+OK\r\n");
    let (_, effects) = whole(relay, Side::Server, b"+OK in\r\n");
    assert_eq!(
        sent(&effects, Side::App),
        "+OK porter relay ready\r\n+OK already authenticated\r\n+OK already authenticated\r\n"
    );
    assert_eq!(sent(&effects, Side::Server), "STAT\r\n");
}

#[test]
fn an_app_that_floods_before_the_greeting_is_cut_off() {
    let (_, effects) = whole(at_user(), Side::App, &vec![b'x'; 70 * 1024]);
    assert_eq!(
        effects,
        vec![Effect::Close(RelayEnd::Failed(RelayFault::Protocol))]
    );
}

#[test]
fn a_greeting_that_is_not_ok_ends_the_relay() {
    let (_, effects) = whole(
        relay(Tls::Implicit, password()),
        Side::Server,
        b"-ERR go away\r\n",
    );
    assert_eq!(
        effects,
        vec![Effect::Close(RelayEnd::Failed(RelayFault::Protocol))]
    );
}

#[test]
fn a_side_closing_ends_the_relay_by_where_it_stood() {
    let (_, effects) = ready().step(Input::Closed(Side::Server));
    assert_eq!(effects, vec![Effect::Close(RelayEnd::Finished)]);
    let (_, effects) = at_user().step(Input::Closed(Side::Server));
    assert_eq!(
        effects,
        vec![Effect::Close(RelayEnd::Failed(RelayFault::Protocol))]
    );
    let (_, effects) = at_user().step(Input::Closed(Side::App));
    assert_eq!(effects, vec![Effect::Close(RelayEnd::Finished)]);
}

#[test]
fn the_credential_never_shows_in_debug_output() {
    let (_, effects) = whole(at_user(), Side::Server, b"+OK\r\n");
    assert!(!format!("{effects:?}").contains("hunter2"));
}
