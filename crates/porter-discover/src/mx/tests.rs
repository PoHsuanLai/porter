//! MX leads, ported from `mx_records` in mailo's `crates/mail-proto/tests/discover.rs`, with
//! mailo's brand table replaced by provider files' `matching`.

use super::*;
use crate::testing::{mx, name};
use porter_provider::ProviderSpec;

fn spec(id: &str, domains: &[&str], suffixes: &[&str]) -> ProviderSpec {
    let toml = format!(
        r#"id = "{id}"
label = "{id}"
mark = "{id}"
[auth]
kind = "local_runtime"
[discovery]
kind = "fixed"
[matching]
domains = {domains:?}
mx_suffixes = {suffixes:?}
[[capability]]
family = "imap"
kind = "mail"
v = {{ access = "read_write", send = "present", delta = "poll", transport = "imap", labels = "folders" }}
"#
    );
    toml::from_str(&toml).unwrap_or_else(|e| panic!("{e}"))
}

fn set() -> ProviderSet {
    ProviderSet::layered(
        vec![
            spec("google", &["gmail.com"], &["google.com", "googlemail.com"]),
            spec(
                "microsoft",
                &["outlook.com"],
                &["mail.protection.outlook.com"],
            ),
        ],
        vec![],
    )
}

#[test]
fn an_mx_at_google_leads_to_the_google_provider_by_its_nearest_host() {
    let records = [
        mx(10, "alt1.aspmx.l.google.com."),
        mx(1, "aspmx.l.google.com."),
    ];
    let leads = provider_leads(&set(), &name("example.test"), &records);
    assert_eq!(
        leads,
        vec![ProviderLead {
            provider: ProviderId::parse("google").expect("id"),
            via: DomainMatch::Mx,
            exchanger: Some(name("aspmx.l.google.com")),
        }]
    );
}

#[test]
fn an_mx_at_a_tenant_host_leads_to_microsoft() {
    let records = [mx(0, "example-test.mail.protection.outlook.com")];
    let leads = provider_leads(&set(), &name("example.test"), &records);
    assert_eq!(leads[0].provider.as_str(), "microsoft");
}

#[test]
fn a_listed_domain_leads_before_an_mx_hint_and_names_no_host() {
    let records = [mx(0, "aspmx.l.google.com")];
    let leads = provider_leads(&set(), &name("outlook.com"), &records);
    assert_eq!(leads.len(), 2);
    assert_eq!(leads[0].provider.as_str(), "microsoft");
    assert_eq!(leads[0].via, DomainMatch::Domain);
    assert_eq!(leads[0].exchanger, None);
    assert_eq!(leads[1].provider.as_str(), "google");
}

#[test]
fn a_lookalike_host_is_not_within_the_suffix() {
    let records = [mx(0, "evil-google.com")];
    assert!(provider_leads(&set(), &name("example.test"), &records).is_empty());
}

#[test]
fn hosts_are_ordered_by_preference_then_name() {
    let records = [
        mx(10, "b.example.test"),
        mx(10, "a.example.test"),
        mx(1, "z.example.test"),
    ];
    let hosts: Vec<String> = mx_hosts(&records).iter().map(ToString::to_string).collect();
    assert_eq!(
        hosts,
        ["z.example.test", "a.example.test", "b.example.test"]
    );
    assert!(mx_hosts(&[]).is_empty());
}

#[test]
fn the_ispdb_is_asked_about_each_parent_of_an_unknown_host_but_never_the_domain_itself() {
    let candidates = |host: &str, domain: &str| -> Vec<String> {
        ispdb_candidates(&name(host), &name(domain))
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert_eq!(
        candidates("mx3.hosting.test", "example.test"),
        ["hosting.test"]
    );
    assert_eq!(
        candidates("a.b.co.uk", "example.test"),
        ["b.co.uk", "co.uk"]
    );
    assert!(candidates("mx.example.test", "example.test").is_empty());
    assert!(candidates("hosting.test", "example.test").is_empty());
}
