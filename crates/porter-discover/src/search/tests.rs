//! The search, ported from mailo's `crates/mail-runtime/tests/discover.rs`: the same scenarios
//! over a table `Http` and `Dns` instead of loopback TLS servers and reqwest. (The TLS-trust and
//! redirect scenarios belong to the `Http` implementation's tests, not to the search.)

use super::*;
use crate::testing::{ADDRESS, Records, Table, mx, respond, srv};
use porter_provider::ProviderSpec;

const OWN: &str =
    "https://autoconfig.example.test/mail/config-v1.1.xml?emailaddress=someone%40example.test";
const WELL_KNOWN: &str = "https://example.test/.well-known/autoconfig/mail/config-v1.1.xml?emailaddress=someone%40example.test";
const ISPDB: &str = "https://autoconfig.thunderbird.net/v1.1/example.test";

fn document(incoming: &str, outgoing: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<clientConfig version="1.1"><emailProvider id="x">
  <incomingServer type="imap"><hostname>{incoming}</hostname><port>993</port>
    <socketType>SSL</socketType><username>%EMAILADDRESS%</username>
    <authentication>password-cleartext</authentication></incomingServer>
  <outgoingServer type="smtp"><hostname>{outgoing}</hostname><port>465</port>
    <socketType>SSL</socketType><username>%EMAILADDRESS%</username>
    <authentication>password-cleartext</authentication></outgoingServer>
</emailProvider></clientConfig>"#
    )
}

const NOTHING: &str = "";

fn no_providers() -> ProviderSet {
    ProviderSet::layered(vec![], vec![])
}

fn google() -> ProviderSet {
    let spec: ProviderSpec = toml::from_str(
        r#"id = "google"
label = "Google"
mark = "google"
[auth]
kind = "local_runtime"
[discovery]
kind = "fixed"
[matching]
domains = ["gmail.com"]
mx_suffixes = ["google.com"]
[[capability]]
family = "imap"
kind = "mail"
v = { access = "read_write", send = "present", delta = "poll", transport = "imap", labels = "folders" }
"#,
    )
    .expect("spec");
    ProviderSet::layered(vec![spec], vec![])
}

async fn run(http: &Table, dns: &Records, set: &ProviderSet) -> Result<Outcome, NotFound> {
    discover_mail(http, dns, set, ADDRESS).await
}

fn servers(outcome: Outcome) -> Found {
    match outcome {
        Outcome::Servers(found) => found,
        other => panic!("{other:?}"),
    }
}

fn first_url(found: &Found) -> String {
    found.endpoints[0].url.to_string()
}

#[tokio::test]
async fn the_domains_own_document_is_asked_first_and_stops_the_search() {
    let http = Table::default().get(
        OWN,
        respond(200, &document("imap.example.test", "smtp.example.test")),
    );
    let dns = Records::default();
    let found = servers(run(&http, &dns, &no_providers()).await.expect("found"));
    assert_eq!(found.source, Source::Autoconfig);
    assert_eq!(first_url(&found), "imaps://imap.example.test:993");
    assert_eq!(
        found.endpoints[1].url.to_string(),
        "smtps://smtp.example.test:465"
    );
    assert_eq!(
        http.seen(),
        [format!("GET {OWN}")],
        "stopped at the first usable answer"
    );
    assert!(
        dns.asked.lock().expect("asked").is_empty(),
        "DNS was not needed"
    );
}

#[tokio::test]
async fn the_well_known_path_is_tried_next() {
    let http = Table::default().get(OWN, respond(404, NOTHING)).get(
        WELL_KNOWN,
        respond(200, &document("mail.example.test", "mail.example.test")),
    );
    let found = servers(
        run(&http, &Records::default(), &no_providers())
            .await
            .expect("found"),
    );
    assert_eq!(first_url(&found), "imaps://mail.example.test:993");
}

#[tokio::test]
async fn the_own_hosts_get_the_address_and_the_ispdb_is_asked_for_the_domain_only() {
    let http = Table::default().get(
        ISPDB,
        respond(200, &document("imap.provider.test", "smtp.provider.test")),
    );
    let found = servers(
        run(&http, &Records::default(), &no_providers())
            .await
            .expect("found"),
    );
    assert_eq!(first_url(&found), "imaps://imap.provider.test:993");
    let seen = http.seen();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(
        seen[0].contains("?emailaddress=someone%40example.test"),
        "{seen:?}"
    );
    assert!(
        seen[1].contains("?emailaddress=someone%40example.test"),
        "{seen:?}"
    );
    assert!(
        !seen[2].contains("emailaddress") && !seen[2].contains("%40"),
        "{seen:?}"
    );
}

#[tokio::test]
async fn a_document_naming_no_usable_server_is_recorded_and_the_search_goes_on() {
    let starttls_free = r#"<clientConfig version="1.1"><emailProvider id="x">
  <incomingServer type="imap"><hostname>imap.example.test</hostname><port>143</port>
    <socketType>plain</socketType></incomingServer>
  <outgoingServer type="smtp"><hostname>smtp.example.test</hostname><port>25</port>
    <socketType>plain</socketType></outgoingServer>
</emailProvider></clientConfig>"#;
    let http = Table::default().get(OWN, respond(200, starttls_free));
    let dns = Records::default();
    let err = run(&http, &dns, &no_providers())
        .await
        .expect_err("nothing usable");
    assert!(!err.offline());
    assert!(
        err.tried
            .iter()
            .any(|t| t.what == OWN && t.miss == Miss::Unusable(DiscoverFault::NoServers)),
        "{err}"
    );
    assert!(
        http.seen().iter().any(|s| s.contains("thunderbird")),
        "later sources were asked"
    );
    assert!(
        dns.asked
            .lock()
            .expect("asked")
            .iter()
            .any(|q| q.starts_with("MX"))
    );
}

#[tokio::test]
async fn srv_records_serve_when_no_document_exists() {
    let mut dns = Records::default();
    dns.srv.insert(
        "_imaps._tcp.example.test".to_owned(),
        vec![srv(0, 1, 993, "imap.example.test.")],
    );
    dns.srv.insert(
        "_submissions._tcp.example.test".to_owned(),
        vec![srv(0, 1, 465, "smtp.example.test.")],
    );
    let found = servers(
        run(&Table::default(), &dns, &no_providers())
            .await
            .expect("found"),
    );
    assert_eq!(found.source, Source::Srv);
    assert_eq!(first_url(&found), "imaps://imap.example.test:993");
}

#[tokio::test]
async fn a_provider_that_lists_the_domain_answers_without_any_network() {
    let http = Table::default();
    let dns = Records::default();
    let outcome = discover_mail(&http, &dns, &google(), "me@gmail.com")
        .await
        .expect("lead");
    let Outcome::Provider(lead) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(lead.provider.as_str(), "google");
    assert_eq!(lead.via, DomainMatch::Domain);
    assert!(http.seen().is_empty());
    assert!(dns.asked.lock().expect("asked").is_empty());
}

#[tokio::test]
async fn an_mx_at_google_is_the_google_provider_without_asking_the_ispdb_about_google() {
    let http = Table::default();
    let mut dns = Records::default();
    dns.mx.insert(
        "example.test".to_owned(),
        vec![
            mx(5, "alt1.aspmx.l.google.com."),
            mx(1, "aspmx.l.google.com."),
        ],
    );
    let outcome = run(&http, &dns, &google()).await.expect("lead");
    let Outcome::Provider(lead) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(lead.provider.as_str(), "google");
    assert_eq!(lead.via, DomainMatch::Mx);
    assert!(!http.seen().iter().any(|s| s.contains("google")));
}

#[tokio::test]
async fn an_mx_at_an_unknown_host_leads_to_that_hosts_ispdb_document() {
    let http = Table::default().get(
        "https://autoconfig.thunderbird.net/v1.1/hosting.test",
        respond(200, &document("imap.hosting.test", "smtp.hosting.test")),
    );
    let mut dns = Records::default();
    dns.mx
        .insert("example.test".to_owned(), vec![mx(10, "mx3.hosting.test.")]);
    let found = servers(run(&http, &dns, &no_providers()).await.expect("found"));
    assert_eq!(found.source, Source::Mx);
    assert_eq!(first_url(&found), "imaps://imap.hosting.test:993");
}

#[tokio::test]
async fn nothing_answering_reads_as_offline() {
    let dns = Records {
        down: true,
        ..Records::default()
    };
    let err = run(&Table::default(), &dns, &no_providers())
        .await
        .expect_err("offline");
    assert!(err.offline(), "{err}");
    assert!(err.to_string().contains("offline"), "{err}");
}

#[tokio::test]
async fn a_server_error_or_an_html_page_is_not_a_configuration() {
    let http = Table::default().get(OWN, respond(503, NOTHING)).get(
        WELL_KNOWN,
        respond(200, "<html><body>Welcome</body></html>"),
    );
    let err = run(&http, &Records::default(), &no_providers())
        .await
        .expect_err("none");
    assert!(
        err.tried
            .iter()
            .any(|t| t.what == OWN && t.miss == Miss::Malformed)
    );
    assert!(
        err.tried
            .iter()
            .any(|t| t.what == WELL_KNOWN && t.miss == Miss::Malformed)
    );
    assert!(!err.offline());
}

#[tokio::test]
async fn a_document_over_the_cap_is_refused_unread() {
    let big = format!(
        "{}{}",
        document("i.example.test", "s.example.test"),
        " ".repeat(MAX_DOCUMENT)
    );
    let http = Table::default().get(OWN, respond(200, &big));
    let err = run(&http, &Records::default(), &no_providers())
        .await
        .expect_err("none");
    assert!(
        err.tried
            .iter()
            .any(|t| t.what == OWN && t.miss == Miss::Malformed)
    );
}

#[tokio::test]
async fn something_that_is_not_an_address_is_malformed_and_asks_nothing() {
    let http = Table::default();
    let err = discover_mail(&http, &Records::default(), &no_providers(), "nobody")
        .await
        .expect_err("no domain");
    assert_eq!(err.tried[0].miss, Miss::Malformed);
    assert!(http.seen().is_empty());
}
