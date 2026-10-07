//! Every former mailo preset resolves to a shipped provider file.
//!
//! The cases are copied from mailo (`~/mailo` at origin/master; mailo deletes its copies in E3):
//! - `crates/mail-domain/src/presets.rs`, tests `gmail_domains_and_case`, `unknown_and_malformed`,
//!   `a_mail_exchanger_in_a_providers_domain_gives_that_providers_preset`,
//!   `only_the_two_known_issuers_are_recognised` (`is_personal_microsoft`), `last_at_wins`;
//! - `crates/mail-core/src/provider.rs`, test `the_preset_wins_and_a_lookalike_host_does_not`
//!   (the host-suffix brand rows: Microsoft, Fastmail, iCloud, Yahoo, Google).
//!
//! mailo has no domain table for Fastmail, iCloud, Yahoo or GMX (its brand is read from the
//! incoming host); their domains and MX suffixes are the providers' public facts.

use porter_provider::{DomainMatch, DomainName, ProviderSet, ProviderSpec, parse_provider};
use std::path::PathBuf;

fn set() -> ProviderSet {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../providers");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("providers directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    let specs = paths
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path).expect("readable");
            parse_provider(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        })
        .collect();
    ProviderSet::layered(specs, vec![])
}

fn name(text: &str) -> DomainName {
    DomainName::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// The id of the provider that claims the address, with how, or none.
fn resolve(set: &ProviderSet, address: &str, mx: &[&str]) -> Option<(String, DomainMatch)> {
    let (_, domain) = address.rsplit_once('@')?;
    let mx: Vec<DomainName> = mx.iter().map(|h| name(h)).collect();
    set.claiming(&DomainName::parse(domain).ok()?, &mx)
        .first()
        .map(|(spec, how)| (spec.id.to_string(), *how))
}

type Case = (
    &'static str,
    &'static str,
    &'static [&'static str],
    Option<(&'static str, DomainMatch)>,
);

#[test]
fn every_former_preset_address_and_mx_resolves_to_its_file() {
    use DomainMatch::{Domain, Mx};
    const CASES: &[Case] = &[
        // Microsoft, personal and work: one multi-tenant file claims both (mailo's preset
        // was work-only and `is_personal_microsoft` kept the personal domains out; porter signs
        // both in through one client, design/31 §3.2).
        (
            "outlook",
            "me@outlook.com",
            &[],
            Some(("microsoft", Domain)),
        ),
        (
            "hotmail",
            "me@Hotmail.com",
            &[],
            Some(("microsoft", Domain)),
        ),
        ("live", "me@live.com", &[], Some(("microsoft", Domain))),
        ("msn", "me@msn.com", &[], Some(("microsoft", Domain))),
        (
            "tenant fallback domain",
            "me@onmicrosoft.com",
            &[],
            Some(("microsoft", Domain)),
        ),
        // A tenant host is claimed by its domain suffix with no MX at all, and a lookalike is not.
        (
            "tenant host, no MX",
            "me@contoso.onmicrosoft.com",
            &[],
            Some(("microsoft", Domain)),
        ),
        ("notonmicrosoft.com", "me@notonmicrosoft.com", &[], None),
        (
            "onmicrosoft.com.evil.test",
            "me@onmicrosoft.com.evil.test",
            &[],
            None,
        ),
        (
            "tenant host with its MX (the domain wins)",
            "me@contoso.onmicrosoft.com",
            &["contoso-onmicrosoft-com.mail.protection.outlook.com"],
            Some(("microsoft", Domain)),
        ),
        (
            "company domain on Microsoft 365 (issuer by MX, outlook.com)",
            "me@firm.example",
            &["firm-example.mail.protection.outlook.com"],
            Some(("microsoft", Mx)),
        ),
        (
            "outlook.com MX itself",
            "me@firm.example",
            &["mx.outlook.com"],
            Some(("microsoft", Mx)),
        ),
        (
            "last at wins",
            "\"odd@name\"@outlook.com",
            &[],
            Some(("microsoft", Domain)),
        ),
        // The brand rows.
        (
            "fastmail",
            "me@fastmail.com",
            &[],
            Some(("fastmail", Domain)),
        ),
        (
            "fastmail.fm",
            "me@fastmail.fm",
            &[],
            Some(("fastmail", Domain)),
        ),
        (
            "fastmail custom domain",
            "me@firm.example",
            &["in1-smtp.messagingengine.com"],
            Some(("fastmail", Mx)),
        ),
        ("icloud", "me@icloud.com", &[], Some(("icloud", Domain))),
        ("me.com", "me@me.com", &[], Some(("icloud", Domain))),
        ("mac.com", "me@mac.com", &[], Some(("icloud", Domain))),
        (
            "icloud custom domain",
            "me@firm.example",
            &["mx01.mail.icloud.com"],
            Some(("icloud", Mx)),
        ),
        ("yahoo", "me@yahoo.com", &[], Some(("yahoo", Domain))),
        ("ymail", "me@ymail.com", &[], Some(("yahoo", Domain))),
        (
            "yahoo custom domain",
            "me@firm.example",
            &["mta5.am0.yahoodns.net"],
            Some(("yahoo", Mx)),
        ),
        ("gmx.com", "me@gmx.com", &[], Some(("gmx", Domain))),
        ("gmx.de", "me@gmx.de", &[], Some(("gmx", Domain))),
        (
            "gmx custom domain",
            "me@firm.example",
            &["mx00.gmx.net"],
            Some(("gmx", Mx)),
        ),
        // A listed domain wins over an MX hint.
        (
            "domain before MX",
            "me@icloud.com",
            &["mx.outlook.com"],
            Some(("icloud", Domain)),
        ),
        // Google (W5c): mailo's gmail/googlemail preset and its google.com MX rule.
        ("gmail", "someone@gmail.com", &[], Some(("google", Domain))),
        (
            "googlemail",
            "someone@GoogleMail.COM",
            &[],
            Some(("google", Domain)),
        ),
        (
            "google MX",
            "me@firm.example",
            &["aspmx.l.google.com"],
            Some(("google", Mx)),
        ),
        (
            "google MX, the other domain",
            "me@firm.example",
            &["gmr-smtp-in.l.google.com", "alt1.aspmx.l.googlemail.com"],
            Some(("google", Mx)),
        ),
        (
            "lookalike google MX",
            "me@firm.example",
            &["notgoogle.com"],
            None,
        ),
        (
            "google as a prefix label",
            "me@firm.example",
            &["google.com.example.test"],
            None,
        ),
        // Unknown and malformed (mailo `unknown_and_malformed`); `generic-*` claim nothing by
        // name, discovery finds them.
        ("unknown domain", "someone@example.com", &[], None),
        (
            "subdomain of a brand domain",
            "someone@sub.icloud.com",
            &[],
            None,
        ),
        ("lookalike domain", "someone@gmail.com.evil.test", &[], None),
        ("not an address", "not-an-address", &[], None),
        ("empty", "", &[], None),
        ("no local part", "@outlook.com", &[], None),
        // Lookalike MX hosts (mailo's `notgoogle.com` and `google.com.example.test` rows).
        (
            "lookalike outlook MX",
            "me@firm.example",
            &["evil-outlook.com"],
            None,
        ),
        (
            "outlook as a prefix label",
            "me@firm.example",
            &["outlook.com.example.test"],
            None,
        ),
        (
            "lookalike messagingengine",
            "me@firm.example",
            &["notmessagingengine.com"],
            None,
        ),
        (
            "lookalike yahoodns",
            "me@firm.example",
            &["evil-yahoodns.net"],
            None,
        ),
    ];
    let set = set();
    let mut failures = Vec::new();
    for (case, address, mx, expect) in CASES {
        // Addresses with no usable domain have no claim; `rsplit_once` splits at the last `@`.
        let got = match address.rsplit_once('@') {
            Some(("", _)) => None,
            _ => resolve(&set, address, mx),
        };
        let want = expect.map(|(id, how)| (id.to_owned(), how));
        if got != want {
            failures.push(format!("{case}: got {got:?}, expected {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The file whose capabilities name `host` as an endpoint host (mailo's host-suffix brand rows).
fn file_serving(set: &ProviderSet, host: &str) -> Option<String> {
    let host_of = |endpoint: &str| -> String {
        let rest = endpoint
            .split_once("://")
            .map_or(endpoint, |(_, rest)| rest);
        rest.split(['/', ':']).next().unwrap_or("").to_owned()
    };
    let serves = |spec: &&ProviderSpec| {
        spec.capabilities
            .iter()
            .filter_map(|row| row.endpoint.as_ref())
            .any(|endpoint| host_of(&endpoint.0) == host)
    };
    set.specs()
        .iter()
        .find(serves)
        .map(|spec| spec.id.to_string())
}

#[test]
fn the_hosts_mailo_branded_are_the_hosts_the_files_declare() {
    // mail-core `provider.rs` cases: (host, brand). Google's mail row names Gmail's host.
    const CASES: &[(&str, &str, Option<&str>)] = &[
        ("office365 host", "outlook.office365.com", Some("microsoft")),
        ("fastmail host", "imap.fastmail.com", Some("fastmail")),
        ("icloud host", "imap.mail.me.com", Some("icloud")),
        ("yahoo host", "imap.mail.yahoo.com", Some("yahoo")),
        ("gmail host", "imap.gmail.com", Some("google")),
        ("lookalike", "imap.fastmail.com.evil.test", None),
        ("anything else", "imap.example.test", None),
    ];
    let set = set();
    for (case, host, expect) in CASES {
        assert_eq!(file_serving(&set, host).as_deref(), *expect, "{case}");
    }
}

#[test]
fn a_listed_domain_is_claimed_by_one_file_only() {
    let set = set();
    let mut seen: Vec<(String, String)> = Vec::new();
    for spec in set.specs() {
        for domain in spec
            .matching
            .domains
            .iter()
            .chain(&spec.matching.mx_suffixes)
        {
            seen.push((domain.to_string(), spec.id.to_string()));
        }
    }
    for (i, (domain, id)) in seen.iter().enumerate() {
        for (other, other_id) in &seen[i + 1..] {
            assert!(
                domain != other || id == other_id,
                "{domain} is claimed by {id} and {other_id}"
            );
        }
    }
}

#[test]
fn the_fallbacks_claim_nothing() {
    let set = set();
    for id in ["generic-imap", "generic-dav", "generic-jmap"] {
        let spec = set
            .specs()
            .iter()
            .find(|spec| spec.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} ships"));
        assert!(spec.matching.domains.is_empty(), "{id}");
        assert!(spec.matching.mx_suffixes.is_empty(), "{id}");
    }
}
