//! Which provider an address belongs to: the domains it serves and the MX hosts that give it
//! away (a company domain on Microsoft 365 has an MX under `mail.protection.outlook.com`).
//! mailo's built-in brand table becomes these rows.

use porter_core::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A DNS name in lower case: dot-separated labels of `[a-z0-9-]`, none empty or over 63 bytes,
/// at most 253 in all. A trailing root dot is dropped.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DomainName(String);

impl DomainName {
    /// The name written as `text`, lower-cased, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let name = text.strip_suffix('.').unwrap_or(text).to_ascii_lowercase();
        let label_ok = |label: &str| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        match name.len() <= 253 && name.split('.').all(label_ok) {
            true => Ok(Self(name)),
            false => Err(CoreError::MalformedId {
                what: "domain name",
                text: text.to_owned(),
            }),
        }
    }

    /// The name's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this name is `suffix` or a name under it, compared label by label:
    /// `mx.outlook.com` is within `outlook.com`, `evil-outlook.com` is not.
    pub fn is_within(&self, suffix: &DomainName) -> bool {
        self.0 == suffix.0
            || self
                .0
                .strip_suffix(suffix.as_str())
                .is_some_and(|head| head.ends_with('.'))
    }
}

impl TryFrom<String> for DomainName {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<DomainName> for String {
    fn from(name: DomainName) -> String {
        name.0
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The `[matching]` table of a provider file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Matching {
    /// Address domains the provider serves, exactly (`outlook.com`).
    pub domains: Vec<DomainName>,
    /// MX hosts under these names mark a domain as the provider's.
    pub mx_suffixes: Vec<DomainName>,
    /// Address domains at or under these names (`onmicrosoft.com` claims
    /// `contoso.onmicrosoft.com`), compared label by label.
    #[serde(default)]
    pub domain_suffixes: Vec<DomainName>,
}

/// How a provider came to claim an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainMatch {
    /// The address's domain is one the provider lists.
    Domain,
    /// One of the domain's MX hosts is under a suffix the provider lists.
    Mx,
}

impl Matching {
    /// Whether the provider claims an address at `domain` whose MX hosts are `mx_hosts`; a
    /// listed domain wins over an MX hint.
    pub fn claims(&self, domain: &DomainName, mx_hosts: &[DomainName]) -> Option<DomainMatch> {
        if self.domains.contains(domain) || self.domain_suffixes.iter().any(|s| domain.is_within(s))
        {
            return Some(DomainMatch::Domain);
        }
        mx_hosts
            .iter()
            .any(|host| self.mx_suffixes.iter().any(|suffix| host.is_within(suffix)))
            .then_some(DomainMatch::Mx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> DomainName {
        DomainName::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    fn microsoft() -> Matching {
        Matching {
            domains: vec![name("outlook.com"), name("hotmail.com")],
            mx_suffixes: vec![name("mail.protection.outlook.com")],
            domain_suffixes: vec![name("onmicrosoft.com")],
        }
    }

    #[test]
    fn names_parse_to_lower_case_labels() {
        assert_eq!(name("Mail.Example.ORG.").as_str(), "mail.example.org");
        for bad in [
            "", ".", "a..b", "-a.org", "a-.org", "a b.org", "a_b.org", "ü.org",
        ] {
            assert!(DomainName::parse(bad).is_err(), "{bad:?}");
        }
        assert!(DomainName::parse(&"a".repeat(64)).is_err());
    }

    #[test]
    fn a_name_is_within_a_suffix_only_at_a_label_boundary() {
        const CASES: &[(&str, &str, &str, bool)] = &[
            ("equal", "outlook.com", "outlook.com", true),
            (
                "a subdomain",
                "mx1.mail.protection.outlook.com",
                "mail.protection.outlook.com",
                true,
            ),
            (
                "a lookalike prefix",
                "evil-outlook.com",
                "outlook.com",
                false,
            ),
            (
                "a lookalike suffix",
                "outlook.com.evil.test",
                "outlook.com",
                false,
            ),
            (
                "a parent is not within",
                "outlook.com",
                "mail.outlook.com",
                false,
            ),
        ];
        for (case, host, suffix, within) in CASES {
            assert_eq!(name(host).is_within(&name(suffix)), *within, "{case}");
        }
    }

    #[test]
    fn a_provider_claims_by_domain_then_by_mx() {
        let claims = |domain: &str, mx: &[&str]| {
            let mx: Vec<DomainName> = mx.iter().map(|h| name(h)).collect();
            microsoft().claims(&name(domain), &mx)
        };
        assert_eq!(claims("outlook.com", &[]), Some(DomainMatch::Domain));
        assert_eq!(
            claims("example.org", &["example-org.mail.protection.outlook.com"]),
            Some(DomainMatch::Mx)
        );
        assert_eq!(
            claims("outlook.com", &["example-org.mail.protection.outlook.com"]),
            Some(DomainMatch::Domain),
            "a listed domain wins"
        );
        assert_eq!(claims("example.org", &["mx.example.org"]), None);
        assert_eq!(claims("sub.outlook.com", &[]), None, "domains are exact");
        assert_eq!(
            claims("contoso.onmicrosoft.com", &[]),
            Some(DomainMatch::Domain),
            "a suffix claims what is under it"
        );
        assert_eq!(claims("onmicrosoft.com", &[]), Some(DomainMatch::Domain));
        assert_eq!(claims("notonmicrosoft.com", &[]), None);
        assert_eq!(claims("onmicrosoft.com.evil.test", &[]), None);
    }
}
