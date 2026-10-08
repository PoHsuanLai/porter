//! Provider files claim addresses: a `[matching]` table parses, round-trips, and the set says
//! which providers claim a domain.

use porter_provider::{DomainMatch, DomainName, ProviderSet, parse_provider};

const MICROSOFT: &str = r#"
id = "microsoft"
label = "Microsoft"
mark = "microsoft"

[auth]
kind = "oauth_pkce"
issuer = "microsoft"

[discovery]
kind = "autoconfig"

[matching]
domains = ["outlook.com", "hotmail.com"]
mx_suffixes = ["mail.protection.outlook.com"]

[[capability]]
family = "imap"
kind = "mail"
v = { access = "read_write", send = "present", delta = "poll", transport = "imap", labels = "folders" }
"#;

const GENERIC: &str = r#"
id = "generic-imap"
label = "Other mail"
mark = "generic"

[auth]
kind = "password"

[discovery]
kind = "autoconfig"

[[capability]]
family = "imap"
kind = "mail"
v = { access = "read_write", send = "present", delta = "push", transport = "imap", labels = "folders" }
"#;

fn name(text: &str) -> DomainName {
    DomainName::parse(text).expect("domain")
}

#[test]
fn a_matching_table_parses_and_a_file_without_one_claims_nothing() {
    let microsoft = parse_provider(MICROSOFT).expect("parses");
    assert_eq!(
        microsoft.matching.domains,
        vec![name("outlook.com"), name("hotmail.com")]
    );
    let text = toml::to_string(&microsoft).expect("toml");
    assert_eq!(parse_provider(&text).as_ref(), Ok(&microsoft));
    let generic = parse_provider(GENERIC).expect("parses");
    assert!(generic.matching.domains.is_empty() && generic.matching.mx_suffixes.is_empty());
}

#[test]
fn the_set_names_the_providers_that_claim_an_address_domain() {
    let set = ProviderSet::layered(
        vec![
            parse_provider(GENERIC).expect("parses"),
            parse_provider(MICROSOFT).expect("parses"),
        ],
        vec![],
    );
    let ids = |domain: &str, mx: &[&str]| -> Vec<(String, DomainMatch)> {
        let mx: Vec<DomainName> = mx.iter().map(|h| name(h)).collect();
        set.claiming(&name(domain), &mx)
            .into_iter()
            .map(|(spec, how)| (spec.id.to_string(), how))
            .collect()
    };
    assert_eq!(
        ids("outlook.com", &[]),
        vec![("microsoft".into(), DomainMatch::Domain)]
    );
    assert_eq!(
        ids("example.org", &["example-org.mail.protection.outlook.com"]),
        vec![("microsoft".into(), DomainMatch::Mx)]
    );
    assert!(ids("example.org", &["mx.example.org"]).is_empty());
}
