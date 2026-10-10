use super::*;
use crate::step::{Effect, Input, RelayEnd, Relaying, Side};
use crate::testing::{bytewise, sent, whole};
use porter_core::{
    CapabilityKind, EndpointUrl, Family, LoginName, RelayAuth, SecretText, ServiceEndpoint, Tls,
};

fn plan(url: &str, tls: Tls, auth: RelayAuth) -> RelayPlan {
    RelayPlan::new(
        ServiceEndpoint {
            family: Family::WebDav,
            url: EndpointUrl::parse(url).expect("url"),
            tls,
            login: LoginName("ada".into()),
        },
        CapabilityKind::Storage,
        auth,
    )
}

/// A WebDAV server at the origin's root, so every path is the endpoint's: these tests are about
/// the head; where a narrower endpoint reaches is `reach`'s, and the last test here.
fn cloud() -> RelayPlan {
    plan(
        "https://cloud.example.org/",
        Tls::Implicit,
        RelayAuth::Password(SecretText::new("hunter2")),
    )
}

// base64 of "ada:hunter2"
const BASIC: &str = "Authorization: Basic YWRhOmh1bnRlcjI=";

fn rewritten(request: &str) -> Result<String, RelayFault> {
    rewrite_head(request.as_bytes(), &cloud()).map(|bytes| String::from_utf8(bytes).expect("text"))
}

#[test]
fn a_head_is_rewritten_to_the_origin_with_the_relays_credential() {
    const CASES: &[(&str, &str, &str)] = &[
        (
            "origin form, host as the endpoint's",
            "PROPFIND /remote.php/dav/files/ada/ HTTP/1.1\r\nHost: cloud.example.org\r\nDepth: 1\r\n\r\n",
            "PROPFIND /remote.php/dav/files/ada/ HTTP/1.1\r\nHost: cloud.example.org\r\nDepth: 1\r\n",
        ),
        (
            "host with the default port, any case",
            "GET /x HTTP/1.1\r\nHOST: Cloud.Example.org:443\r\n\r\n",
            "GET /x HTTP/1.1\r\nHost: cloud.example.org\r\n",
        ),
        (
            "no host at all gets the endpoint's",
            "GET /x HTTP/1.0\r\n\r\n",
            "GET /x HTTP/1.0\r\nHost: cloud.example.org\r\n",
        ),
        (
            "absolute form of the same origin becomes origin form",
            "GET https://cloud.example.org/a?b=1 HTTP/1.1\r\nHost: cloud.example.org\r\n\r\n",
            "GET /a?b=1 HTTP/1.1\r\nHost: cloud.example.org\r\n",
        ),
        (
            "absolute form with no path",
            "OPTIONS https://cloud.example.org HTTP/1.1\r\n\r\n",
            "OPTIONS / HTTP/1.1\r\nHost: cloud.example.org\r\n",
        ),
        (
            "the app's credentials are dropped, whatever the case",
            "GET /x HTTP/1.1\r\nAuthorization: Basic c3RvbGVu\r\nPROXY-AUTHORIZATION: x\r\nauthorization: Bearer y\r\nAccept: */*\r\n\r\n",
            "GET /x HTTP/1.1\r\nHost: cloud.example.org\r\nAccept: */*\r\n",
        ),
        (
            "a length body keeps its header",
            "PUT /f HTTP/1.1\r\nContent-Length: 5\r\n\r\n",
            "PUT /f HTTP/1.1\r\nHost: cloud.example.org\r\nContent-Length: 5\r\n",
        ),
        (
            "chunked",
            "PUT /f HTTP/1.1\r\nTransfer-Encoding: Chunked\r\n\r\n",
            "PUT /f HTTP/1.1\r\nHost: cloud.example.org\r\nTransfer-Encoding: Chunked\r\n",
        ),
    ];
    for (name, request, expected) in CASES {
        let out = rewritten(request).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(out, format!("{expected}{BASIC}\r\n\r\n"), "{name}");
    }
}

#[test]
fn an_anonymous_relay_adds_no_credential_and_drops_the_apps() {
    let linked = plan("https://up.1drv.com/", Tls::Implicit, RelayAuth::Anonymous);
    let out = rewrite_head(
        b"PUT /up/s1 HTTP/1.1\r\nHost: up.1drv.com\r\nAuthorization: Bearer stolen\r\nContent-Length: 3\r\n\r\n",
        &linked,
    )
    .expect("rewritten");
    assert_eq!(
        String::from_utf8(out).expect("text"),
        "PUT /up/s1 HTTP/1.1\r\nHost: up.1drv.com\r\nContent-Length: 3\r\n\r\n"
    );
    // The origin check is the same: another host is refused.
    let foreign = rewrite_head(b"GET /x HTTP/1.1\r\nHost: evil.example\r\n\r\n", &linked);
    assert_eq!(foreign, Err(RelayFault::ForeignOrigin));
}

#[test]
fn a_token_is_presented_as_a_bearer_and_a_custom_port_stays_in_host() {
    let token = plan(
        "https://cloud.example.org:8443/",
        Tls::Implicit,
        RelayAuth::AccessToken(SecretText::new("tok")),
    );
    let out = rewrite_head(
        b"GET /x HTTP/1.1\r\nHost: cloud.example.org:8443\r\n\r\n",
        &token,
    )
    .expect("rewritten");
    assert_eq!(
        String::from_utf8(out).expect("text"),
        "GET /x HTTP/1.1\r\nHost: cloud.example.org:8443\r\nAuthorization: Bearer tok\r\n\r\n"
    );
    let v6 = plan(
        "http://[::1]:8080/",
        Tls::Plain,
        RelayAuth::AccessToken(SecretText::new("t")),
    );
    let out = rewrite_head(b"GET /x HTTP/1.1\r\nHost: [::1]:8080\r\n\r\n", &v6).expect("v6");
    assert!(
        String::from_utf8(out)
            .expect("text")
            .contains("Host: [::1]:8080\r\n")
    );
}

#[test]
fn another_origin_or_an_ambiguous_head_is_refused() {
    const CASES: &[(&str, &str, RelayFault)] = &[
        (
            "another host in the target",
            "GET https://evil.example/x HTTP/1.1\r\nHost: cloud.example.org\r\n\r\n",
            RelayFault::ForeignOrigin,
        ),
        (
            "the endpoint's host with another port in the target",
            "GET https://cloud.example.org:444/x HTTP/1.1\r\n\r\n",
            RelayFault::ForeignOrigin,
        ),
        (
            "plain http to an https endpoint's host and default port",
            "GET http://cloud.example.org/x HTTP/1.1\r\n\r\n",
            RelayFault::ForeignOrigin,
        ),
        (
            "user info in the target",
            "GET https://ada@cloud.example.org/x HTTP/1.1\r\n\r\n",
            RelayFault::ForeignOrigin,
        ),
        (
            "another scheme",
            "GET ftp://cloud.example.org/x HTTP/1.1\r\n\r\n",
            RelayFault::ForeignOrigin,
        ),
        (
            "a host header of another origin",
            "GET /x HTTP/1.1\r\nHost: evil.example\r\n\r\n",
            RelayFault::ForeignOrigin,
        ),
        (
            "a suffix is not the host",
            "GET /x HTTP/1.1\r\nHost: cloud.example.org.evil.example\r\n\r\n",
            RelayFault::ForeignOrigin,
        ),
        (
            "two hosts",
            "GET /x HTTP/1.1\r\nHost: cloud.example.org\r\nHost: evil.example\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "both framings",
            "PUT /x HTTP/1.1\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "a repeated length",
            "PUT /x HTTP/1.1\r\nContent-Length: 3\r\nContent-Length: 3\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "a signed length",
            "PUT /x HTTP/1.1\r\nContent-Length: +3\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "another transfer coding",
            "PUT /x HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "line folding",
            "GET /x HTTP/1.1\r\nX-A: b\r\n c\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "space before the colon",
            "GET /x HTTP/1.1\r\nHost : cloud.example.org\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "connect",
            "CONNECT cloud.example.org:443 HTTP/1.1\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "upgrade",
            "GET /x HTTP/1.1\r\nUpgrade: websocket\r\n\r\n",
            RelayFault::Protocol,
        ),
        ("http 2", "GET /x HTTP/2\r\n\r\n", RelayFault::Protocol),
        ("no version", "GET /x\r\n\r\n", RelayFault::Protocol),
        (
            "a bare lf",
            "GET /x HTTP/1.1\nHost: cloud.example.org\r\n\r\n",
            RelayFault::Protocol,
        ),
        (
            "a nul",
            "GET /x HTTP/1.1\r\nX: a\0b\r\n\r\n",
            RelayFault::Protocol,
        ),
    ];
    for (name, request, fault) in CASES {
        assert_eq!(rewritten(request), Err(*fault), "{name}");
    }
}

fn effects_close(effects: &[Effect], end: RelayEnd) -> bool {
    effects.contains(&Effect::Close(end))
}

#[test]
fn heads_and_bodies_pass_wherever_the_reads_split_and_the_next_head_is_found() {
    let request = b"PUT /f HTTP/1.1\r\nHost: cloud.example.org\r\nContent-Length: 4\r\n\r\nGET GET /g HTTP/1.1\r\nHost: cloud.example.org\r\n\r\n";
    let (relay, effects) = bytewise(HttpRelay::new(cloud()), Side::App, request);
    let to_server = sent(&effects, Side::Server);
    // The body "GET " passes as body; the second head is rewritten.
    assert!(
        to_server
            .contains("Content-Length: 4\r\nAuthorization: Basic YWRhOmh1bnRlcjI=\r\n\r\nGET ")
    );
    assert!(to_server.contains("GET /g HTTP/1.1\r\nHost: cloud.example.org\r\n"));
    assert_eq!(to_server.matches("Authorization: Basic").count(), 2);
    assert_eq!(relay.phase, HttpPhase::Head);
    assert!(!effects_close(&effects, RelayEnd::Finished));
}

#[test]
fn a_chunked_body_is_followed_by_the_next_request() {
    let request = "POST /f HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nGET \r\n0\r\n\r\nGET /g HTTP/1.1\r\n\r\n";
    for split in [1, 7, 60, request.len()] {
        let mut relay = HttpRelay::new(cloud());
        let mut all = Vec::new();
        for part in request.as_bytes().chunks(split) {
            let (next, effects) = whole(relay, Side::App, part);
            relay = next;
            all.extend(effects);
        }
        let to_server = sent(&all, Side::Server);
        assert!(
            to_server.contains("4\r\nGET \r\n0\r\n\r\nGET /g HTTP/1.1\r\n"),
            "{split}: {to_server:?}"
        );
        assert_eq!(
            to_server.matches("Authorization: Basic").count(),
            2,
            "{split}"
        );
    }
}

#[test]
fn a_refused_head_sends_the_app_a_4xx_and_ends_the_relay() {
    let (_, effects) = whole(
        HttpRelay::new(cloud()),
        Side::App,
        b"GET /x HTTP/1.1\r\nHost: evil.example\r\n\r\n",
    );
    assert!(sent(&effects, Side::App).starts_with("HTTP/1.1 403 "));
    assert_eq!(sent(&effects, Side::Server), "");
    assert!(effects_close(
        &effects,
        RelayEnd::Failed(RelayFault::ForeignOrigin)
    ));
    let (_, effects) = whole(
        HttpRelay::new(cloud()),
        Side::App,
        b"PUT /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
    );
    assert!(effects_close(
        &effects,
        RelayEnd::Failed(RelayFault::Protocol)
    ));
    let long = vec![b'a'; 70 * 1024];
    let (_, effects) = whole(HttpRelay::new(cloud()), Side::App, &long);
    assert!(effects_close(
        &effects,
        RelayEnd::Failed(RelayFault::Protocol)
    ));
}

#[test]
fn responses_pass_unchanged_and_blank_lines_before_a_request_are_skipped() {
    let (relay, effects) = whole(
        HttpRelay::new(cloud()),
        Side::Server,
        b"HTTP/1.1 200 OK\r\n\r\n",
    );
    assert_eq!(sent(&effects, Side::App), "HTTP/1.1 200 OK\r\n\r\n");
    let (_, effects) = whole(relay, Side::App, b"\r\n\r\nGET /x HTTP/1.1\r\n\r\n");
    assert!(
        sent(&effects, Side::Server).starts_with("GET /x HTTP/1.1\r\nHost: cloud.example.org\r\n")
    );
    let (_, effects) = HttpRelay::new(cloud()).step(Input::Closed(Side::App));
    assert!(effects_close(&effects, RelayEnd::Finished));
}

#[test]
fn a_contacts_grants_relay_refuses_the_files_and_ocs_and_passes_its_address_books() {
    let mut contacts = plan(
        "https://cloud.example.org/remote.php/dav/addressbooks/users/ada/",
        Tls::Implicit,
        RelayAuth::Password(SecretText::new("hunter2")),
    );
    contacts.endpoint.family = Family::CardDav;
    contacts.kind = CapabilityKind::Contacts;
    for refused in [
        "PROPFIND /remote.php/dav/files/ada/ HTTP/1.1\r\nDepth: 1\r\n\r\n",
        "GET /ocs/v2.php/core/apppassword HTTP/1.1\r\nOCS-APIRequest: true\r\n\r\n",
        "POST /ocs/v2.php/core/apppassword/rotate HTTP/1.1\r\nOCS-APIRequest: true\r\n\r\n",
        "GET https://cloud.example.org/remote.php/dav/files/ada/x HTTP/1.1\r\n\r\n",
    ] {
        let (_, effects) = whole(
            HttpRelay::new(contacts.clone()),
            Side::App,
            refused.as_bytes(),
        );
        assert!(
            sent(&effects, Side::App).starts_with("HTTP/1.1 403 "),
            "{refused}"
        );
        assert_eq!(sent(&effects, Side::Server), "", "{refused}");
        assert!(
            effects_close(&effects, RelayEnd::Failed(RelayFault::ForeignOrigin)),
            "{refused}"
        );
    }
    let (_, effects) = whole(
        HttpRelay::new(contacts),
        Side::App,
        b"PROPFIND /remote.php/dav/addressbooks/users/ada/contacts/ HTTP/1.1\r\nDepth: 1\r\n\r\n",
    );
    assert!(sent(&effects, Side::Server).starts_with(
        "PROPFIND /remote.php/dav/addressbooks/users/ada/contacts/ HTTP/1.1\r\nHost: cloud.example.org\r\nDepth: 1\r\n"
    ));
    assert_eq!(sent(&effects, Side::App), "");
}
