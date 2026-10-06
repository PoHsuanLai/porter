//! Which scopes a sign-in asks for, which scope each audience is refreshed with, and whether an
//! address is Microsoft's personal kind.
//!
//! The IMAP and SMTP scopes, `offline_access` and `openid`, and the personal-domain list are
//! ported from mailo's `presets.rs` (`~/mailo/crates/mail-domain/src/presets.rs`:
//! `MICROSOFT_SCOPES`, `is_personal_microsoft`, `issuer_for_server`), the issuer and brand half
//! only.

use porter_core::capability::CapabilityKind;
use porter_core::{Audience, TokenKind};

/// Exchange Online's IMAP scope (a delegated permission, not the application-only family).
pub(super) const IMAP_SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All";
/// Exchange Online's SMTP submission scope: without it an account syncs and cannot send.
pub(super) const SMTP_SCOPE: &str = "https://outlook.office.com/SMTP.Send";
/// Graph's resource, and the scope that names every Graph permission already consented to.
pub(super) const GRAPH_DEFAULT: &str = "https://graph.microsoft.com/.default";

const GRAPH: &str = "https://graph.microsoft.com/";

/// The scopes for the kinds that are on, mail's first so a code is redeemed for Exchange (see
/// `porter_oauth::redeem_scope`). `User.Read` is always asked: it is how the account's address
/// is read. `offline_access` is what returns a refresh token.
pub(super) fn scopes_for(kinds: &[CapabilityKind]) -> Vec<String> {
    let on = |kind| kinds.contains(&kind);
    let graph = |name: &str| format!("{GRAPH}{name}");
    let mut scopes = Vec::new();
    if on(CapabilityKind::Mail) {
        scopes.extend([IMAP_SCOPE.to_owned(), SMTP_SCOPE.to_owned()]);
    }
    scopes.push(graph("User.Read"));
    let per_kind = [
        (CapabilityKind::Calendar, "Calendars.ReadWrite"),
        (CapabilityKind::Contacts, "Contacts.ReadWrite"),
        (CapabilityKind::Tasks, "Tasks.ReadWrite"),
        (CapabilityKind::Notes, "Notes.ReadWrite"),
        (CapabilityKind::Storage, "Files.ReadWrite.AppFolder"),
    ];
    scopes.extend(
        per_kind
            .iter()
            .filter(|(k, _)| on(*k))
            .map(|(_, s)| graph(s)),
    );
    scopes.extend(["offline_access".to_owned(), "openid".to_owned()]);
    scopes
}

/// What an audience is refreshed with and the form of the token it gets: IMAP and SMTP take an
/// XOAUTH2 token for the Exchange resource, Graph takes a bearer.
pub(super) fn audience_scope(
    audience: &Audience,
    graph_endpoint: &str,
) -> Option<(String, TokenKind)> {
    match audience.0.as_str() {
        "imap" => Some((IMAP_SCOPE.to_owned(), TokenKind::Xoauth2)),
        "smtp" => Some((SMTP_SCOPE.to_owned(), TokenKind::Xoauth2)),
        a if a == "graph" || a.trim_end_matches('/') == graph_endpoint.trim_end_matches('/') => {
            Some((GRAPH_DEFAULT.to_owned(), TokenKind::Bearer))
        }
        _ => None,
    }
}

/// Whether the scopes name only Graph, so the code's token is already Graph's.
pub(super) fn graph_only(scopes: &[String]) -> bool {
    !scopes
        .iter()
        .any(|s| s.starts_with("https://outlook.office.com/"))
}

/// Which kind of Microsoft account an address belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountClass {
    /// A consumer account (outlook.com, hotmail.com, live.com, msn.com): no organisation, so no
    /// tenant to consent.
    Personal,
    /// A work or school account, in an organisation's tenant.
    Work,
}

/// The class of the account `address` names. The rule is mailo's: the four consumer domains are
/// personal, anything else is a tenant's.
pub fn classify(address: &str) -> AccountClass {
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
    match domain.trim().to_ascii_lowercase().as_str() {
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => AccountClass::Personal,
        _ => AccountClass::Work,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CapabilityKind as K;

    #[test]
    fn scopes_follow_the_kinds_that_are_on() {
        let strs = |kinds: &[K]| scopes_for(kinds);
        assert_eq!(
            strs(&[K::Mail, K::Calendar]),
            [
                IMAP_SCOPE,
                SMTP_SCOPE,
                "https://graph.microsoft.com/User.Read",
                "https://graph.microsoft.com/Calendars.ReadWrite",
                "offline_access",
                "openid",
            ]
        );
        assert_eq!(
            strs(&[K::Storage, K::Notes]),
            [
                "https://graph.microsoft.com/User.Read",
                "https://graph.microsoft.com/Notes.ReadWrite",
                "https://graph.microsoft.com/Files.ReadWrite.AppFolder",
                "offline_access",
                "openid",
            ]
        );
        assert_eq!(
            strs(&[]),
            [
                "https://graph.microsoft.com/User.Read",
                "offline_access",
                "openid"
            ]
        );
    }

    #[test]
    fn an_exchange_first_sign_in_redeems_for_exchange_and_a_graph_only_one_for_graph() {
        let mail = scopes_for(&[K::Mail, K::Tasks]);
        let redeemed = porter_oauth::redeem_scope(&mail).expect("two resources");
        assert!(redeemed.contains("IMAP.AccessAsUser.All"));
        assert!(!redeemed.contains("graph.microsoft.com"));
        assert!(!graph_only(&mail));
        let files = scopes_for(&[K::Storage]);
        assert_eq!(porter_oauth::redeem_scope(&files), None);
        assert!(graph_only(&files));
    }

    #[test]
    fn audiences_map_to_one_resource_and_a_token_form() {
        const G: &str = "https://graph.microsoft.com";
        const CASES: &[(&str, Option<(&str, TokenKind)>)] = &[
            ("imap", Some((IMAP_SCOPE, TokenKind::Xoauth2))),
            ("smtp", Some((SMTP_SCOPE, TokenKind::Xoauth2))),
            ("graph", Some((GRAPH_DEFAULT, TokenKind::Bearer))),
            (
                "https://graph.microsoft.com",
                Some((GRAPH_DEFAULT, TokenKind::Bearer)),
            ),
            (
                "https://graph.microsoft.com/",
                Some((GRAPH_DEFAULT, TokenKind::Bearer)),
            ),
            ("https://graph.evil.test", None),
            ("webdav", None),
        ];
        for (audience, want) in CASES {
            let got = audience_scope(&Audience((*audience).into()), G);
            assert_eq!(got, want.map(|(s, k)| (s.to_owned(), k)), "{audience}");
        }
    }

    #[test]
    fn personal_and_work_addresses_are_told_apart_by_domain() {
        const CASES: &[(&str, AccountClass)] = &[
            ("me@outlook.com", AccountClass::Personal),
            ("me@Hotmail.com", AccountClass::Personal),
            ("me@live.com", AccountClass::Personal),
            ("me@msn.com", AccountClass::Personal),
            ("me@contoso.onmicrosoft.com", AccountClass::Work),
            ("me@firm.example", AccountClass::Work),
            ("me@notoutlook.com", AccountClass::Work),
            ("not an address", AccountClass::Work),
        ];
        for (address, want) in CASES {
            assert_eq!(classify(address), *want, "{address}");
        }
    }
}
