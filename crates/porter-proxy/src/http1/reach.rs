//! Where under its origin an HTTP relay may go: the plan's reach, which the service decided when
//! it planned the relay (`porter_service`'s `reach`: a Nextcloud contacts grant stays under the
//! address books, and so on), applied to each request's target here. The relay decides nothing
//! about families or providers.
//!
//! A target is compared with its percent escapes decoded; one that could mean another path to
//! the server than it does to the relay (a `.` or `..` segment, an escaped `/` or `\`, a `\`, an
//! escaped NUL, a malformed escape) is refused when the reach is narrower than the origin.

use crate::fault::RelayFault;
use porter_core::{ReachMethods, RelayPlan};

/// `%XX` escapes decoded to bytes; `None` for a malformed one (a `%` without two hex digits),
/// which a server may read as it likes.
fn decoded(text: &str) -> Option<Vec<u8>> {
    porter_ids::percent::is_well_formed(text).then(|| porter_ids::percent::decode_bytes(text))
}

/// `path` decoded, when the server cannot read it as another path than the relay does.
fn plain_path(path: &str) -> Option<Vec<u8>> {
    let lower = path.to_ascii_lowercase();
    if path.contains('\\') || ["%2f", "%5c", "%00"].iter().any(|e| lower.contains(e)) {
        return None;
    }
    let bytes = decoded(path)?;
    let dotted = bytes
        .split(|b| *b == b'/')
        .any(|segment| segment == b"." || segment == b"..");
    (!dotted).then_some(bytes)
}

fn under(path: &[u8], prefix: &[u8]) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
}

/// Whether the request `method target` (origin-form, or `*`) with this `Depth` stays where the
/// plan reaches; `ForeignOrigin` when it does not.
pub(super) fn check(
    plan: &RelayPlan,
    method: &str,
    target: &str,
    depth: Option<&str>,
) -> Result<(), RelayFault> {
    if target == "*" || plan.reach.is_origin() {
        return Ok(());
    }
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let path = plain_path(path).ok_or(RelayFault::ForeignOrigin)?;
    let allowed = |methods: ReachMethods| match methods {
        ReachMethods::Any => true,
        ReachMethods::ReadProperties => {
            method.eq_ignore_ascii_case("PROPFIND") && depth.is_some_and(|d| d == "0")
        }
        // a way of limiting methods a newer porter adds: never taken as unlimited
        _ => false,
    };
    let reached = plan
        .reach
        .paths()
        .iter()
        .any(|r| under(&path, r.prefix()) && allowed(r.methods()));
    match reached {
        true => Ok(()),
        false => Err(RelayFault::ForeignOrigin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{
        CapabilityKind, EndpointUrl, Family, LoginName, PathReach, Reach, RelayAuth, SecretText,
        ServiceEndpoint, Tls,
    };

    fn plan(reach: Reach) -> RelayPlan {
        RelayPlan::new(
            ServiceEndpoint {
                family: Family::CardDav,
                url: EndpointUrl::parse(
                    "https://cloud.example.org/remote.php/dav/addressbooks/users/ada/",
                )
                .expect("url"),
                tls: Tls::Implicit,
                login: LoginName("ada".into()),
            },
            CapabilityKind::Contacts,
            RelayAuth::Password(SecretText::new("app-password")),
            reach,
        )
    }

    /// What the service plans for a contacts grant on Nextcloud: the address books, and the
    /// principals collection beside them for `PROPFIND` with `Depth: 0`.
    fn books() -> Reach {
        Reach::under(vec![
            PathReach::new(
                b"/remote.php/dav/addressbooks/users/ada".to_vec(),
                ReachMethods::Any,
            ),
            PathReach::new(
                b"/remote.php/dav/principals".to_vec(),
                ReachMethods::ReadProperties,
            ),
        ])
    }

    #[test]
    fn a_relay_reaching_the_origin_checks_no_path() {
        let whole = plan(Reach::origin());
        assert_eq!(check(&whole, "GET", "/anything/%2f..", None), Ok(()));
    }

    #[test]
    fn a_contacts_relay_on_nextcloud_stays_under_the_address_books() {
        let contacts = plan(books());
        const CASES: &[(&str, &str, Option<&str>, bool)] = &[
            ("the home", "PROPFIND", Some("1"), true),
            ("the home without its slash", "PROPFIND", Some("1"), true),
            ("a card", "PUT", None, true),
            ("a query on a card", "GET", None, true),
            ("the files", "PROPFIND", Some("1"), false),
            ("a file", "GET", None, false),
            ("ocs app password", "GET", None, false),
            ("ocs rotate", "POST", None, false),
            ("the notes app", "GET", None, false),
            ("another user's books", "PROPFIND", Some("1"), false),
            ("a lookalike sibling", "GET", None, false),
            ("dot dot out", "GET", None, false),
            ("escaped dot dot out", "GET", None, false),
            ("escaped slash", "GET", None, false),
            ("backslash", "GET", None, false),
            ("malformed escape", "GET", None, false),
            ("the principal's properties", "PROPFIND", Some("0"), true),
            ("the principals listed", "PROPFIND", Some("1"), false),
            ("the principals with no depth", "PROPFIND", None, false),
            ("a write to the principal", "PROPPATCH", Some("0"), false),
            ("the server root", "PROPFIND", Some("0"), false),
            ("asterisk", "OPTIONS", None, true),
        ];
        let targets = [
            "/remote.php/dav/addressbooks/users/ada/",
            "/remote.php/dav/addressbooks/users/ada",
            "/remote.php/dav/addressbooks/users/ada/contacts/card-1.vcf",
            "/remote.php/dav/addressbooks/users/ada/contacts/card-1.vcf?export",
            "/remote.php/dav/files/ada/",
            "/remote.php/dav/files/ada/secret.txt",
            "/ocs/v2.php/core/apppassword",
            "/ocs/v2.php/core/apppassword/rotate",
            "/index.php/apps/notes/api/v1/notes",
            "/remote.php/dav/addressbooks/users/bob/",
            "/remote.php/dav/addressbooks/users/adam/",
            "/remote.php/dav/addressbooks/users/ada/../../../files/ada/",
            "/remote.php/dav/addressbooks/users/ada/%2e%2e/%2E%2E/%2e%2e/files/ada/",
            "/remote.php/dav/addressbooks/users/ada%2f..%2f..%2f..%2ffiles/ada/",
            "/remote.php/dav/addressbooks/users/ada/..\\..\\files",
            "/remote.php/dav/addressbooks/users/ada/%zz",
            "/remote.php/dav/principals/users/ada/",
            "/remote.php/dav/principals/users/",
            "/remote.php/dav/principals/users/ada/",
            "/remote.php/dav/principals/users/ada/",
            "/remote.php/dav/",
            "*",
        ];
        for ((name, method, depth, allowed), target) in CASES.iter().zip(targets) {
            let got = check(&contacts, method, target, *depth);
            let want = match allowed {
                true => Ok(()),
                false => Err(RelayFault::ForeignOrigin),
            };
            assert_eq!(got, want, "{name}: {method} {target}");
        }
    }
}
