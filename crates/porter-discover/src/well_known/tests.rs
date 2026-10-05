//! `.well-known` and the JMAP session.

use super::*;
use crate::testing::{ADDRESS, Table, name, respond};
use porter_http::{Header, Status};

fn login() -> LoginName {
    LoginName(ADDRESS.to_owned())
}

fn url(text: &str) -> EndpointUrl {
    EndpointUrl::parse(text).expect("url")
}

fn redirect(status: u16, location: &str) -> HttpResponse {
    HttpResponse {
        status: Status(status),
        headers: vec![Header::new("Location", location)],
        body: Vec::new(),
    }
}

#[test]
fn the_urls_by_kind() {
    let domain = name("example.test");
    let urls = |kind| -> Vec<String> {
        well_known_urls(&domain, kind)
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert_eq!(
        urls(CapabilityKind::Calendar),
        ["https://example.test/.well-known/caldav"]
    );
    assert_eq!(
        urls(CapabilityKind::Tasks),
        ["https://example.test/.well-known/caldav"]
    );
    assert_eq!(
        urls(CapabilityKind::Contacts),
        ["https://example.test/.well-known/carddav"]
    );
    assert_eq!(
        urls(CapabilityKind::Mail),
        ["https://example.test/.well-known/jmap"]
    );
    for none in [
        CapabilityKind::Storage,
        CapabilityKind::Llm,
        CapabilityKind::Identity,
    ] {
        assert!(urls(none).is_empty(), "{none:?}");
    }
}

#[test]
fn a_redirect_names_the_service_root() {
    let asked = url("https://example.test/.well-known/caldav");
    let cases = [
        (
            "https://dav.example.test/remote.php/dav",
            "https://dav.example.test/remote.php/dav",
        ),
        ("/remote.php/dav/", "https://example.test/remote.php/dav/"),
    ];
    for (location, want) in cases {
        let found = well_known_found(
            CapabilityKind::Calendar,
            &asked,
            &redirect(301, location),
            &login(),
        )
        .expect("found");
        assert_eq!(found.endpoints[0].url.to_string(), want, "{location}");
        assert_eq!(found.endpoints[0].family, Family::CalDav);
        assert_eq!(found.endpoints[0].tls, Tls::Implicit);
        assert_eq!(found.source, Source::WellKnown);
    }
}

#[test]
fn a_redirect_to_plain_http_or_nowhere_is_unreadable() {
    let asked = url("https://example.test/.well-known/carddav");
    for response in [
        redirect(302, "http://example.test/dav"),
        redirect(302, "ftp://example.test/dav"),
        redirect(302, "not a url"),
        respond(302, ""),
    ] {
        assert_eq!(
            well_known_found(CapabilityKind::Contacts, &asked, &response, &login()),
            Err(DiscoverFault::Unreadable)
        );
    }
}

#[test]
fn an_answer_in_place_is_the_service_and_a_missing_one_is_none() {
    let asked = url("https://example.test/.well-known/carddav");
    for status in [200, 207, 401] {
        let found = well_known_found(
            CapabilityKind::Contacts,
            &asked,
            &respond(status, ""),
            &login(),
        )
        .expect("found");
        assert_eq!(found.endpoints[0].url, asked);
        assert_eq!(found.endpoints[0].family, Family::CardDav);
    }
    assert_eq!(
        well_known_found(
            CapabilityKind::Contacts,
            &asked,
            &respond(404, ""),
            &login()
        ),
        Err(DiscoverFault::NoServers)
    );
    assert_eq!(
        well_known_found(
            CapabilityKind::Contacts,
            &asked,
            &respond(500, ""),
            &login()
        ),
        Err(DiscoverFault::Unreadable)
    );
    assert_eq!(
        well_known_found(CapabilityKind::Storage, &asked, &respond(200, ""), &login()),
        Err(DiscoverFault::NoServers)
    );
}

#[tokio::test]
async fn the_fetch_asks_the_url_and_reads_its_redirect() {
    let http = Table::default().get(
        "https://example.test/.well-known/caldav",
        redirect(301, "/dav/"),
    );
    let found = discover_well_known(
        &http,
        &name("example.test"),
        CapabilityKind::Calendar,
        &login(),
    )
    .await
    .expect("found");
    assert_eq!(found.endpoints[0].url.path(), "/dav/");
    assert_eq!(
        discover_well_known(
            &Table::default(),
            &name("example.test"),
            CapabilityKind::Calendar,
            &login()
        )
        .await,
        Err(DiscoverFault::Unreachable)
    );
}

#[test]
fn the_recorded_jmap_session_names_its_endpoint_and_capabilities() {
    let found =
        parse_jmap_session(include_str!("../../fixtures/jmap-session.json")).expect("found");
    assert_eq!(found.source, Source::JmapSession);
    let endpoint = &found.endpoints[0];
    assert_eq!(endpoint.family, Family::Jmap);
    assert_eq!(endpoint.url.to_string(), "https://jmap.example.test/api/");
    assert_eq!(endpoint.tls, Tls::Implicit);
    assert_eq!(endpoint.login.0, "me@example.test");
    let kinds: Vec<CapabilityKind> = found.claims.iter().map(|c| c.offer.kind()).collect();
    assert_eq!(kinds, [CapabilityKind::Mail, CapabilityKind::Contacts]);
    assert!(
        found
            .claims
            .iter()
            .all(|c| c.provenance == Provenance::Discovered)
    );
    let Offer::Present(Capability::Mail(mail)) = &found.claims[0].offer else {
        panic!("{:?}", found.claims[0])
    };
    assert_eq!(mail.transport, MailTransport::Jmap);
    assert_eq!(mail.send, Offered::Present);
}

#[test]
fn mail_without_the_submission_capability_cannot_send_and_calendars_are_read() {
    let json = r#"{"capabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:calendars":{}},"apiUrl":"https://j.example.test/api"}"#;
    let found = parse_jmap_session(json).expect("found");
    let Offer::Present(Capability::Mail(mail)) = &found.claims[0].offer else {
        panic!("{:?}", found.claims[0])
    };
    assert_eq!(mail.send, Offered::Absent);
    assert_eq!(found.claims[1].offer.kind(), CapabilityKind::Calendar);
    assert_eq!(found.endpoints[0].login.0, "", "no username in the session");
}

#[test]
fn a_loopback_jmap_over_http_is_plain_and_any_other_is_refused() {
    let session = |api: &str| {
        format!(
            r#"{{"capabilities":{{"urn:ietf:params:jmap:core":{{}}}},"apiUrl":"{api}","username":"u"}}"#
        )
    };
    let local = parse_jmap_session(&session("http://127.0.0.1:8080/jmap")).expect("found");
    assert_eq!(local.endpoints[0].tls, Tls::Plain);
    assert!(local.claims.is_empty());
    assert_eq!(
        parse_jmap_session(&session("http://jmap.example.test/api")),
        Err(DiscoverFault::NoServers)
    );
}

#[test]
fn a_session_that_is_not_one_is_unreadable() {
    for text in [
        "",
        "[]",
        "{}",
        r#"{"apiUrl":"https://j.example.test/api"}"#,
        r#"{"capabilities":{"urn:ietf:params:jmap:core":{}}}"#,
        r#"{"capabilities":{"urn:ietf:params:jmap:core":{}},"apiUrl":"nonsense"}"#,
        r#"{"capabilities":{"urn:ietf:params:jmap:core":{}},"apiUrl":3}"#,
    ] {
        assert_eq!(
            parse_jmap_session(text),
            Err(DiscoverFault::Unreadable),
            "{text:?}"
        );
    }
}
