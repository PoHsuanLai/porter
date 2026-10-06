//! The search options and what a document offers beyond IMAP and SMTP: the OAuth2 offer, POP3
//! servers, the `STARTTLS` rule. Table tests over documents shaped like those in mailo's
//! `crates/mail-proto/tests/discover.rs` (MIT OR Apache-2.0, same author): the OAuth2 issuer
//! named only in `<oAuth2>`, OAuth2 on a host nobody maps, POP3 beside and without IMAP.

use crate::found::*;
use crate::search::{Outcome, discover_mail, discover_mail_with};
use crate::testing::{ADDRESS, Records, Table, doc, respond, server};
use crate::{DiscoverFault, parse_autoconfig, parse_autoconfig_with};
use porter_core::{LoginName, Tls};
use porter_provider::ProviderSet;

const OWN: &str =
    "https://autoconfig.example.test/mail/config-v1.1.xml?emailaddress=someone%40example.test";
const ISPDB: &str = "https://autoconfig.thunderbird.net/v1.1/example.test";

fn with_issuer(servers: &[String], issuer: &str) -> String {
    doc(&servers.concat()).replace(
        "</clientConfig>",
        &format!("<oAuth2><issuer>{issuer}</issuer></oAuth2></clientConfig>"),
    )
}

fn oauth_both(host: &str) -> Vec<String> {
    vec![
        server("imap", &format!("imap.{host}"), 993, "SSL", "", &["OAuth2"]),
        server("smtp", &format!("smtp.{host}"), 465, "SSL", "", &["OAuth2"]),
    ]
}

fn pop3_and_smtp() -> String {
    doc(&[
        server("pop3", "pop-plain.example.test", 110, "plain", "", &[]),
        server("pop3", "pop-tls.example.test", 110, "STARTTLS", "", &[]),
        server(
            "pop3",
            "pop.example.test",
            995,
            "SSL",
            "%EMAILLOCALPART%",
            &[],
        ),
        server("smtp", "smtp.example.test", 465, "SSL", "", &[]),
    ]
    .concat())
}

fn report() -> SearchOptions {
    SearchOptions {
        pop3: Pop3::Report,
        ..SearchOptions::default()
    }
}

fn offer_oauth() -> SearchOptions {
    SearchOptions {
        oauth_only: OAuthOnly::Offer,
        ..SearchOptions::default()
    }
}

fn server_at(direction: Direction, host: &str, port: u16) -> OAuthServer {
    OAuthServer {
        direction,
        host: host.to_owned(),
        port,
    }
}

#[test]
fn the_issuer_named_only_in_the_oauth2_element_and_the_servers_that_offer_oauth_are_exposed() {
    let xml = with_issuer(&oauth_both("example.test"), "login.microsoftonline.com");
    let found = parse_autoconfig_with(&xml, ADDRESS, &offer_oauth()).expect("found");
    assert_eq!(
        found.oauth,
        Some(OAuthOffer {
            issuer: Some("login.microsoftonline.com".to_owned()),
            servers: vec![
                server_at(Direction::Incoming, "imap.example.test", 993),
                server_at(Direction::Outgoing, "smtp.example.test", 465),
            ],
        })
    );
    assert_eq!(found.endpoints.len(), 2);
    assert!(found.pop3.is_empty());
}

#[test]
fn oauth_on_an_unknown_host_has_no_issuer_and_the_host_as_written() {
    let both = [
        server(
            "imap",
            "imap.%EMAILDOMAIN%",
            993,
            "SSL",
            "",
            &["OAuth2", "password-cleartext"],
        ),
        server(
            "smtp",
            "smtp.unknown.test",
            465,
            "SSL",
            "",
            &["password-cleartext"],
        ),
    ]
    .concat();
    // The default options: a password endpoint exists, so the offer rides along.
    let found = parse_autoconfig(&doc(&both), ADDRESS).expect("found");
    assert_eq!(
        found.oauth,
        Some(OAuthOffer {
            issuer: None,
            servers: vec![server_at(Direction::Incoming, "imap.example.test", 993)],
        })
    );
}

#[test]
fn an_oauth_only_document_is_a_miss_by_default_and_a_finding_when_offered() {
    let xml = with_issuer(&oauth_both("example.test"), "accounts.example.test");
    assert_eq!(
        parse_autoconfig(&xml, ADDRESS),
        Err(DiscoverFault::NoServers),
        "unchanged"
    );
    let found = parse_autoconfig_with(&xml, ADDRESS, &offer_oauth()).expect("found");
    assert_eq!(
        found.endpoints[0].url.to_string(),
        "imaps://imap.example.test:993"
    );
    assert_eq!(
        found.endpoints[1].url.to_string(),
        "smtps://smtp.example.test:465"
    );
}

#[test]
fn a_document_without_oauth_has_no_offer() {
    let xml = doc(&[
        server("imap", "i.example.test", 993, "SSL", "", &[]),
        server("smtp", "s.example.test", 465, "SSL", "", &[]),
    ]
    .concat());
    assert_eq!(parse_autoconfig(&xml, ADDRESS).expect("found").oauth, None);
}

#[test]
fn pop3_is_listed_never_plain_implicit_first_and_only_on_request() {
    let xml = pop3_and_smtp();
    assert_eq!(
        parse_autoconfig(&xml, ADDRESS),
        Err(DiscoverFault::NoServers),
        "a POP3-only document is a miss by default"
    );
    let found = parse_autoconfig_with(&xml, ADDRESS, &report()).expect("found");
    assert_eq!(
        found.pop3,
        [
            Pop3Server {
                host: "pop.example.test".to_owned(),
                port: 995,
                tls: Tls::Implicit,
                login: LoginName("someone".to_owned()),
            },
            Pop3Server {
                host: "pop-tls.example.test".to_owned(),
                port: 110,
                tls: Tls::StartTls,
                login: LoginName(ADDRESS.to_owned()),
            },
        ]
    );
    let endpoints: Vec<String> = found.endpoints.iter().map(|e| e.url.to_string()).collect();
    assert_eq!(endpoints, ["smtps://smtp.example.test:465"]);
    assert!(found.claims.is_empty(), "no IMAP, so no mailbox claim");
}

#[test]
fn pop3_beside_imap_leaves_the_endpoints_imap_and_smtp() {
    let xml = doc(&[
        server("imap", "imap.example.test", 993, "SSL", "", &[]),
        server("pop3", "pop.example.test", 995, "SSL", "", &[]),
        server("smtp", "smtp.example.test", 465, "SSL", "", &[]),
    ]
    .concat());
    let found = parse_autoconfig_with(&xml, ADDRESS, &report()).expect("found");
    assert_eq!(found.endpoints.len(), 2);
    assert_eq!(found.pop3.len(), 1);
    let ignored = parse_autoconfig(&xml, ADDRESS).expect("found");
    assert_eq!(ignored.endpoints, found.endpoints);
    assert!(ignored.pop3.is_empty());
}

#[test]
fn pop3_without_an_smtp_server_is_still_a_miss() {
    let xml = doc(&server("pop3", "pop.example.test", 995, "SSL", "", &[]));
    assert_eq!(
        parse_autoconfig_with(&xml, ADDRESS, &report()),
        Err(DiscoverFault::NoServers)
    );
}

#[test]
fn starttls_only_is_a_miss_under_try_next_and_a_finding_otherwise() {
    let xml = doc(&[
        server("imap", "imap.example.test", 143, "STARTTLS", "", &[]),
        server("smtp", "smtp.example.test", 587, "STARTTLS", "", &[]),
    ]
    .concat());
    let try_next = SearchOptions {
        starttls_only: StartTlsOnly::TryNext,
        ..SearchOptions::default()
    };
    assert_eq!(
        parse_autoconfig_with(&xml, ADDRESS, &try_next),
        Err(DiscoverFault::NoServers)
    );
    let accepted = parse_autoconfig_with(&xml, ADDRESS, &SearchOptions::default()).expect("found");
    assert_eq!(accepted.endpoints[0].tls, Tls::StartTls);
    assert_eq!(accepted, parse_autoconfig(&xml, ADDRESS).expect("found"));
    // A half-STARTTLS document is a miss too: mailo needed both sides implicit.
    let half = doc(&[
        server("imap", "imap.example.test", 993, "SSL", "", &[]),
        server("smtp", "smtp.example.test", 587, "STARTTLS", "", &[]),
    ]
    .concat());
    assert_eq!(
        parse_autoconfig_with(&half, ADDRESS, &try_next),
        Err(DiscoverFault::NoServers)
    );
}

fn starttls_document() -> String {
    doc(&[
        server("imap", "old.example.test", 143, "STARTTLS", "", &[]),
        server("smtp", "old.example.test", 587, "STARTTLS", "", &[]),
    ]
    .concat())
}

fn tls_document() -> String {
    doc(&[
        server("imap", "imap.isp.test", 993, "SSL", "", &[]),
        server("smtp", "smtp.isp.test", 465, "SSL", "", &[]),
    ]
    .concat())
}

async fn search(http: &Table, options: &SearchOptions) -> Found {
    let providers = ProviderSet::layered(vec![], vec![]);
    match discover_mail_with(http, &Records::default(), &providers, ADDRESS, options).await {
        Ok(Outcome::Servers(found)) => found,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn try_next_skips_a_starttls_document_and_reaches_the_ispdb() {
    let http = Table::default()
        .get(OWN, respond(200, &starttls_document()))
        .get(ISPDB, respond(200, &tls_document()));
    let found = search(
        &http,
        &SearchOptions {
            starttls_only: StartTlsOnly::TryNext,
            ..SearchOptions::default()
        },
    )
    .await;
    assert_eq!(
        found.endpoints[0].url.to_string(),
        "imaps://imap.isp.test:993"
    );
    assert_eq!(
        http.seen().last().map(String::as_str),
        Some(&*format!("GET {ISPDB}"))
    );
}

#[tokio::test]
async fn the_defaults_take_the_starttls_document_and_discover_mail_is_unchanged() {
    let http = Table::default()
        .get(OWN, respond(200, &starttls_document()))
        .get(ISPDB, respond(200, &tls_document()));
    let found = search(&http, &SearchOptions::default()).await;
    assert_eq!(
        found.endpoints[0].url.to_string(),
        "imap://old.example.test:143"
    );
    assert_eq!(http.seen(), [format!("GET {OWN}")]);

    let providers = ProviderSet::layered(vec![], vec![]);
    let plain = discover_mail(&http, &Records::default(), &providers, ADDRESS).await;
    assert_eq!(plain, Ok(Outcome::Servers(found)));
}

#[tokio::test]
async fn a_pop3_only_document_is_returned_under_report_and_skipped_otherwise() {
    let http = Table::default()
        .get(OWN, respond(200, &pop3_and_smtp()))
        .get(ISPDB, respond(200, &tls_document()));
    let found = search(&http, &report()).await;
    assert_eq!(found.pop3.len(), 2);
    assert_eq!(found.endpoints.len(), 1);
    let found = search(&http, &SearchOptions::default()).await;
    assert!(found.pop3.is_empty());
    assert_eq!(
        found.endpoints[0].url.to_string(),
        "imaps://imap.isp.test:993"
    );
}

#[tokio::test]
async fn a_domain_with_only_pop3_srv_records_is_found_under_report_and_not_otherwise() {
    let mut dns = Records::default();
    dns.srv.insert(
        "_pop3s._tcp.example.test".to_owned(),
        vec![crate::testing::srv(0, 0, 995, "pop.example.test")],
    );
    dns.srv.insert(
        "_submissions._tcp.example.test".to_owned(),
        vec![crate::testing::srv(0, 0, 465, "smtp.example.test")],
    );
    let http = Table::default();
    let providers = ProviderSet::layered(vec![], vec![]);
    let found = match discover_mail_with(&http, &dns, &providers, ADDRESS, &report()).await {
        Ok(Outcome::Servers(found)) => found,
        other => panic!("{other:?}"),
    };
    assert_eq!(found.source, Source::Srv);
    assert_eq!(found.pop3.len(), 1);
    assert_eq!(found.endpoints.len(), 1);
    let ignored =
        discover_mail_with(&http, &dns, &providers, ADDRESS, &SearchOptions::default()).await;
    assert!(
        !matches!(ignored, Ok(Outcome::Servers(_))),
        "pop3-only SRV is a miss by default: {ignored:?}"
    );
}
