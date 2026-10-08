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
/// Reading the signed-in person's profile (their address), asked at every sign-in.
pub(super) const USER_READ: &str = "https://graph.microsoft.com/User.Read";

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
    scopes.push(USER_READ.to_owned());
    scopes.extend(
        GRAPH_PER_KIND
            .iter()
            .filter(|(k, _)| on(*k))
            .map(|(_, s)| graph(s)),
    );
    scopes.extend(["offline_access".to_owned(), "openid".to_owned()]);
    scopes
}

/// The Graph permission each kind is signed in with (`scopes_for`), and the only one a token
/// for a grant of that kind carries.
const GRAPH_PER_KIND: [(CapabilityKind, &str); 5] = [
    (CapabilityKind::Calendar, "Calendars.ReadWrite"),
    (CapabilityKind::Contacts, "Contacts.ReadWrite"),
    (CapabilityKind::Tasks, "Tasks.ReadWrite"),
    (CapabilityKind::Notes, "Notes.ReadWrite"),
    (CapabilityKind::Storage, "Files.ReadWrite.AppFolder"),
];

/// The Graph scope a grant for `kind` is refreshed with; none for a kind Graph does not serve.
fn graph_scope(kind: CapabilityKind) -> Option<String> {
    GRAPH_PER_KIND
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, s)| format!("{GRAPH}{s}"))
}

/// Whether `audience` is Graph: its slug, or the Graph endpoint the provider file names.
fn is_graph(audience: &Audience, graph_endpoint: &str) -> bool {
    audience.0 == "graph"
        || audience.0.trim_end_matches('/') == graph_endpoint.trim_end_matches('/')
}

/// What an audience is refreshed with and the form of the token it gets, for porter's own use
/// inside accountd (the add-time probe): IMAP and SMTP take an XOAUTH2 token for the Exchange
/// resource, Graph takes a bearer for every permission consented. No token minted this way
/// leaves accountd: an app's or a relay's comes from [`grant_scope`].
pub(super) fn audience_scope(
    audience: &Audience,
    graph_endpoint: &str,
) -> Option<(String, TokenKind)> {
    match audience.0.as_str() {
        "imap" => Some((IMAP_SCOPE.to_owned(), TokenKind::Xoauth2)),
        "smtp" => Some((SMTP_SCOPE.to_owned(), TokenKind::Xoauth2)),
        _ if is_graph(audience, graph_endpoint) => {
            Some((GRAPH_DEFAULT.to_owned(), TokenKind::Bearer))
        }
        _ => None,
    }
}

/// What a token for a grant of `kind` to `audience` is refreshed with: the kind's own scope and
/// nothing else, never `.default` (which names every Graph permission the account consented to,
/// so a calendar grant's token would reach the account's files). Microsoft's v2 token endpoint
/// takes a subset of the signed-in scopes on a refresh. Mail reaches IMAP and SMTP only; the
/// other kinds reach Graph only. `None` for an audience the kind does not reach.
pub(super) fn grant_scope(
    audience: &Audience,
    kind: CapabilityKind,
    graph_endpoint: &str,
) -> Option<(String, TokenKind)> {
    match (audience.0.as_str(), kind) {
        ("imap", CapabilityKind::Mail) => Some((IMAP_SCOPE.to_owned(), TokenKind::Xoauth2)),
        ("smtp", CapabilityKind::Mail) => Some((SMTP_SCOPE.to_owned(), TokenKind::Xoauth2)),
        _ if is_graph(audience, graph_endpoint) => {
            graph_scope(kind).map(|scope| (scope, TokenKind::Bearer))
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
    fn a_grant_is_refreshed_with_its_own_kinds_scope_and_never_graphs_default() {
        const G: &str = "https://graph.microsoft.com";
        const CAL: &str = "https://graph.microsoft.com/Calendars.ReadWrite";
        const FILES: &str = "https://graph.microsoft.com/Files.ReadWrite.AppFolder";
        type Want = Option<(&'static str, TokenKind)>;
        const CASES: &[(&str, K, Want)] = &[
            ("graph", K::Calendar, Some((CAL, TokenKind::Bearer))),
            (G, K::Calendar, Some((CAL, TokenKind::Bearer))),
            (
                "graph",
                K::Contacts,
                Some((
                    "https://graph.microsoft.com/Contacts.ReadWrite",
                    TokenKind::Bearer,
                )),
            ),
            (
                "graph",
                K::Tasks,
                Some((
                    "https://graph.microsoft.com/Tasks.ReadWrite",
                    TokenKind::Bearer,
                )),
            ),
            (
                "graph",
                K::Notes,
                Some((
                    "https://graph.microsoft.com/Notes.ReadWrite",
                    TokenKind::Bearer,
                )),
            ),
            ("graph", K::Storage, Some((FILES, TokenKind::Bearer))),
            ("imap", K::Mail, Some((IMAP_SCOPE, TokenKind::Xoauth2))),
            ("smtp", K::Mail, Some((SMTP_SCOPE, TokenKind::Xoauth2))),
            ("graph", K::Mail, None),
            ("imap", K::Calendar, None),
            ("smtp", K::Storage, None),
            ("graph", K::Agent, None),
            ("https://graph.evil.test", K::Calendar, None),
        ];
        for (audience, kind, want) in CASES {
            let got = grant_scope(&Audience((*audience).into()), *kind, G);
            assert_eq!(
                got,
                want.map(|(s, k)| (s.to_owned(), k)),
                "{audience} {kind:?}"
            );
            assert!(
                got.is_none_or(|(s, _)| s != GRAPH_DEFAULT && !s.contains(' ')),
                "{audience} {kind:?}: one scope, never .default"
            );
        }
        // Every kind's grant scope is one the sign-in asked for when the kind was on.
        for kind in [K::Calendar, K::Contacts, K::Tasks, K::Notes, K::Storage] {
            let (scope, _) = grant_scope(&Audience("graph".into()), kind, G).expect("graph kind");
            assert!(scopes_for(&[kind]).contains(&scope), "{kind:?}");
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
