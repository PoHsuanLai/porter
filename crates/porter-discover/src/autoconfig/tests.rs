//! Selection, ported from `selection` in mailo's `crates/mail-proto/tests/discover.rs`, with
//! mailo's presets replaced by endpoints; plus the URL list and the username table.

use super::*;
use crate::testing::{ADDRESS, doc, name, server};
use porter_core::Family;

fn found(xml: &str) -> Result<Found, DiscoverFault> {
    parse_autoconfig(xml, ADDRESS)
}

fn urls(found: &Found) -> Vec<String> {
    found.endpoints.iter().map(|e| e.url.to_string()).collect()
}

#[test]
fn the_urls_are_the_domains_own_then_well_known_then_the_ispdb_and_never_carry_the_address() {
    let urls: Vec<String> = autoconfig_urls(&name("example.test"), ADDRESS)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        urls,
        [
            "https://autoconfig.example.test/mail/config-v1.1.xml",
            "https://example.test/.well-known/autoconfig/mail/config-v1.1.xml",
            "https://autoconfig.thunderbird.net/v1.1/example.test",
        ]
    );
}

#[test]
fn the_first_implicit_tls_imap_and_smtp_servers_are_chosen() {
    let found = found(&doc(&[
        server(
            "imap",
            "starttls.example.test",
            143,
            "STARTTLS",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
        server(
            "imap",
            "imap.example.test",
            993,
            "SSL",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
        server(
            "imap",
            "second.example.test",
            993,
            "SSL",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
        server(
            "pop3",
            "pop.example.test",
            995,
            "SSL",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
        server(
            "smtp",
            "submit.example.test",
            587,
            "STARTTLS",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
        server(
            "smtp",
            "smtp.example.test",
            465,
            "SSL",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
    ]
    .concat()))
    .expect("found");
    assert_eq!(
        urls(&found),
        [
            "imaps://imap.example.test:993",
            "smtps://smtp.example.test:465"
        ]
    );
    assert_eq!(found.endpoints[0].family, Family::Imap);
    assert_eq!(found.endpoints[1].family, Family::Smtp);
    assert!(found.endpoints.iter().all(|e| e.tls == Tls::Implicit));
    assert!(found.endpoints.iter().all(|e| e.login.0 == ADDRESS));
    assert_eq!(found.source, Source::Autoconfig);
    assert_eq!(found.claims, vec![imap_claim()]);
}

#[test]
fn pop3_is_never_chosen() {
    let none = found(&doc(&[
        server("pop3", "pop.example.test", 995, "SSL", "", &[]),
        server("smtp", "smtp.example.test", 465, "SSL", "", &[]),
    ]
    .concat()));
    assert_eq!(none, Err(DiscoverFault::NoServers));
}

#[test]
fn starttls_serves_only_when_no_implicit_server_will_and_says_so() {
    let found = found(&doc(&[
        server("imap", "imap.example.test", 143, "STARTTLS", "", &[]),
        server("smtp", "smtp.example.test", 465, "SSL", "", &[]),
        server("smtp", "submit.example.test", 587, "STARTTLS", "", &[]),
    ]
    .concat()))
    .expect("found");
    assert_eq!(
        urls(&found),
        [
            "imap://imap.example.test:143",
            "smtps://smtp.example.test:465"
        ]
    );
    assert_eq!(found.endpoints[0].tls, Tls::StartTls);
    assert_eq!(found.endpoints[1].tls, Tls::Implicit);
}

#[test]
fn plain_servers_are_skipped_even_when_they_are_the_only_ones() {
    let plain = doc(&[
        server("imap", "imap.example.test", 143, "plain", "", &[]),
        server("smtp", "smtp.example.test", 465, "SSL", "", &[]),
    ]
    .concat());
    assert_eq!(found(&plain), Err(DiscoverFault::NoServers));
    let missing_socket = doc(
        "<incomingServer type=\"imap\"><hostname>h.example.test</hostname><port>143</port></incomingServer>",
    );
    assert_eq!(found(&missing_socket), Err(DiscoverFault::NoServers));
}

#[test]
fn a_side_with_no_server_at_all_is_no_servers() {
    let only_imap = doc(&server("imap", "i.example.test", 993, "SSL", "", &[]));
    assert_eq!(found(&only_imap), Err(DiscoverFault::NoServers));
}

#[test]
fn a_challenge_response_only_server_is_not_one_this_can_sign_in_to() {
    let xml = doc(&[
        server(
            "imap",
            "i.example.test",
            993,
            "SSL",
            "",
            &["password-encrypted"],
        ),
        server("smtp", "s.example.test", 465, "SSL", "", &[]),
    ]
    .concat());
    assert_eq!(found(&xml), Err(DiscoverFault::NoServers));
}

#[test]
fn an_oauth_only_server_is_left_to_the_providers_that_know_it() {
    let xml = doc(&[
        server("imap", "i.example.test", 993, "SSL", "", &["OAuth2"]),
        server("smtp", "s.example.test", 465, "SSL", "", &["OAuth2"]),
    ]
    .concat());
    assert_eq!(found(&xml), Err(DiscoverFault::NoServers));
}

#[test]
fn hostnames_and_logins_have_their_placeholders_filled() {
    let found = found(&doc(&[
        server(
            "imap",
            "mail.%EMAILDOMAIN%",
            993,
            "SSL",
            "%EMAILLOCALPART%",
            &[],
        ),
        server("smtp", "mail.%EMAILDOMAIN%", 465, "SSL", "", &[]),
    ]
    .concat()))
    .expect("found");
    assert_eq!(
        urls(&found),
        [
            "imaps://mail.example.test:993",
            "smtps://mail.example.test:465"
        ]
    );
    assert_eq!(found.endpoints[0].login.0, "someone");
    assert_eq!(found.endpoints[1].login.0, ADDRESS);
}

#[test]
fn a_hostname_that_cannot_be_an_endpoint_is_skipped_for_the_next() {
    let found = found(&doc(&[
        server("imap", "evil@host.test", 993, "SSL", "", &[]),
        server("imap", "good.example.test", 993, "SSL", "", &[]),
        server("smtp", "smtp.example.test", 465, "SSL", "", &[]),
    ]
    .concat()))
    .expect("found");
    assert_eq!(
        found.endpoints[0].url.to_string(),
        "imaps://good.example.test:993"
    );
}

#[test]
fn unreadable_documents_are_unreadable() {
    for text in ["", "<html/>", "<clientConfig/>", "not xml"] {
        assert_eq!(found(text), Err(DiscoverFault::Unreadable), "{text:?}");
    }
}

/// The username element, as a table: what the document says, what the account logs in as.
#[test]
fn username_placeholders_resolve_against_the_address() {
    let cases = [
        ("%EMAILADDRESS%", ADDRESS),
        ("", ADDRESS),
        ("  %EMAILADDRESS%  ", ADDRESS),
        ("%EMAILLOCALPART%", "someone"),
        ("%EMAILLOCALPART%@%EMAILDOMAIN%", ADDRESS),
        ("%EMAILDOMAIN%", "example.test"),
        ("%EMAILLOCALPART%.%EMAILDOMAIN%", "someone.example.test"),
        ("fixed-login", "fixed-login"),
    ];
    for (raw, want) in cases {
        assert_eq!(login(raw, ADDRESS).0, want, "{raw:?}");
    }
}

#[test]
fn the_domain_of_an_address_is_after_the_last_at_sign() {
    assert_eq!(domain_of("me@Example.TEST"), Some(name("example.test")));
    assert_eq!(domain_of("a@b@example.test"), Some(name("example.test")));
    for bad in ["", "no-at", "@example.test", "me@", "me@bad domain"] {
        assert_eq!(domain_of(bad), None, "{bad:?}");
    }
}

#[test]
fn the_recorded_ispdb_document_gives_imap_and_smtp_over_tls() {
    let found =
        parse_autoconfig(include_str!("../../fixtures/ispdb-example.xml"), ADDRESS).expect("found");
    assert_eq!(
        urls(&found),
        [
            "imaps://imap.example.test:993",
            "smtps://smtp.example.test:465"
        ]
    );
}
