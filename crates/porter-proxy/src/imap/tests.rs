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
