//! Where under its origin an HTTP relay may go. One origin and one password often serve several
//! kinds of an account (a Nextcloud app password reaches its files, calendars, contacts and the
//! OCS API), so a relay planned for a grant of one kind stays under the path of that kind's
//! endpoint: a contacts grant on Nextcloud reaches `/remote.php/dav/addressbooks/users/ada/` and
//! not `/remote.php/dav/files/` or `/ocs/`.
//!
//! The reach of each family, from the endpoint's path:
//! - CalDAV, CardDAV, WebDAV, Nextcloud Notes: the endpoint's path. CalDAV and CardDAV also
//!   read the account's principal (where a client finds its calendar or address book home): a
//!   `PROPFIND` with `Depth: 0` under the `principals` collection beside the `calendars` or
//!   `addressbooks` tree the endpoint is in (`/remote.php/dav/principals/`).
//! - Every other family: the whole origin, as before. Their bearer is per kind instead (Graph's
//!   and Google's tokens carry the grant's kind's scopes alone, so a calendar token on
//!   `www.googleapis.com` cannot read Drive), and their clients build paths from the origin
//!   (Drive's `/upload/drive/v3`, the Picker's `/v1/sessions`); a JMAP session names its API,
//!   upload and download URLs anywhere on the origin; a relay to a declared auth or linked origin
//!   has the origin's root as its endpoint.
//!
//! A target is compared with its percent escapes decoded; one that could mean another path to
//! the server than it does to the relay (a `.` or `..` segment, an escaped `/` or `\`, a `\`, an
//! escaped NUL, a malformed escape) is refused when the reach is narrower than the origin.

use crate::fault::RelayFault;
use porter_core::{Family, RelayPlan};

/// What a reach lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Methods {
    /// Every method.
    Any,
    /// `PROPFIND` with `Depth: 0`: one resource's properties, no listing.
    ReadProperties,
}

/// One path prefix, decoded and without its trailing `/` (empty: the whole origin).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reach {
    prefix: Vec<u8>,
    methods: Methods,
}

/// Whether `family`'s relays stay under their endpoint's path.
fn confined(family: Family) -> bool {
    matches!(
        family,
        Family::CalDav | Family::CardDav | Family::WebDav | Family::NextcloudNotes
    )
}

/// `%XX` escapes decoded; `None` for a malformed one.
fn decoded(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    Some(out)
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

fn trimmed(mut path: Vec<u8>) -> Vec<u8> {
    while path.last() == Some(&b'/') {
        path.pop();
    }
    path
}

/// The `principals` collection beside the `tree` the endpoint's path is in.
fn principals(prefix: &[u8], tree: &[u8]) -> Option<Vec<u8>> {
    let segments: Vec<&[u8]> = prefix.split(|b| *b == b'/').collect();
    let at = segments.iter().position(|s| *s == tree)?;
    let mut root = segments[..at].join(&b'/');
    root.extend_from_slice(b"/principals");
    Some(root)
}

/// Where a relay planned as `plan` may go.
fn reaches(plan: &RelayPlan) -> Vec<Reach> {
    let family = plan.endpoint.family;
    let path = plan.endpoint.url.path();
    let prefix = decoded(path).map(trimmed).unwrap_or_default();
    if !confined(family) || prefix.is_empty() {
        return vec![Reach {
            prefix: Vec::new(),
            methods: Methods::Any,
        }];
    }
    let mut reach = vec![Reach {
        prefix: prefix.clone(),
        methods: Methods::Any,
    }];
    let tree: Option<&[u8]> = match family {
        Family::CalDav => Some(b"calendars"),
        Family::CardDav => Some(b"addressbooks"),
        _ => None,
    };
    reach.extend(
        tree.and_then(|tree| principals(&prefix, tree))
            .map(|prefix| Reach {
                prefix,
                methods: Methods::ReadProperties,
            }),
    );
    reach
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
    let reach = reaches(plan);
    if target == "*" || reach.iter().any(|r| r.prefix.is_empty()) {
        return Ok(());
    }
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let path = plain_path(path).ok_or(RelayFault::ForeignOrigin)?;
    let allowed = |r: &Reach| match r.methods {
        Methods::Any => true,
        Methods::ReadProperties => {
            method.eq_ignore_ascii_case("PROPFIND") && depth.is_some_and(|d| d == "0")
        }
    };
    match reach.iter().any(|r| under(&path, &r.prefix) && allowed(r)) {
        true => Ok(()),
        false => Err(RelayFault::ForeignOrigin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{
        CapabilityKind, EndpointUrl, LoginName, RelayAuth, SecretText, ServiceEndpoint, Tls,
    };

    fn plan(family: Family, url: &str) -> RelayPlan {
        RelayPlan::new(
            ServiceEndpoint {
                family,
                url: EndpointUrl::parse(url).expect("url"),
                tls: Tls::Implicit,
                login: LoginName("ada".into()),
            },
            CapabilityKind::Contacts,
            RelayAuth::Password(SecretText::new("app-password")),
        )
    }

    const BOOKS: &str = "https://cloud.example.org/remote.php/dav/addressbooks/users/ada/";

    #[test]
    fn a_contacts_relay_on_nextcloud_stays_under_the_address_books() {
        let contacts = plan(Family::CardDav, BOOKS);
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

    #[test]
    fn each_family_reaches_its_own_paths() {
        type Case = (&'static str, Family, &'static str, &'static str, bool);
        const CASES: &[Case] = &[
            (
                "an escaped user name in the endpoint",
                Family::CardDav,
                "https://c.example/remote.php/dav/addressbooks/users/ada%40x.org/",
                "/remote.php/dav/addressbooks/users/ada@x.org/b/",
                true,
            ),
            (
                "files stay in the files",
                Family::WebDav,
                "https://c.example/remote.php/dav/files/ada/",
                "/remote.php/dav/calendars/ada/",
                false,
            ),
            (
                "files reach no principal",
                Family::WebDav,
                "https://c.example/remote.php/dav/files/ada/",
                "/remote.php/dav/principals/users/ada/",
                false,
            ),
            (
                "a calendar home reads its principal",
                Family::CalDav,
                "https://dav.fastmail.example/dav/calendars/user/ada%40x.org/",
                "/dav/principals/user/ada@x.org/",
                true,
            ),
            (
                "notes stay in the notes app",
                Family::NextcloudNotes,
                "https://c.example/index.php/apps/notes/api/v1/",
                "/index.php/apps/notes/api/v1/notes/3",
                true,
            ),
            (
                "notes do not reach the files",
                Family::NextcloudNotes,
                "https://c.example/index.php/apps/notes/api/v1/",
                "/remote.php/dav/files/ada/",
                false,
            ),
            (
                "a lookalike prefix",
                Family::CalDav,
                "https://c.example/remote.php/dav/calendars/ada/",
                "/remote.php/dav/calendars/adam/x.ics",
                false,
            ),
            (
                "google is per token, not per path: drive uploads",
                Family::GoogleDrive,
                "https://www.googleapis.com/drive/v3",
                "/upload/drive/v3/files?uploadType=resumable",
                true,
            ),
            (
                "google is per token, not per path: the picker's sessions",
                Family::GooglePhotosPicker,
                "https://photospicker.googleapis.com/v1/picker",
                "/v1/sessions",
                true,
            ),
            (
                "graph is per token, not per path",
                Family::Graph,
                "https://graph.microsoft.com",
                "/v1.0/me/drive/root/children",
                true,
            ),
            (
                "jmap names its own urls on the origin",
                Family::Jmap,
                "https://api.fastmail.example/jmap/session",
                "/jmap/api/",
                true,
            ),
            (
                "a dav server at its root",
                Family::CalDav,
                "https://caldav.icloud.example",
                "/123/principal/",
                true,
            ),
        ];
        for (name, family, url, target, allowed) in CASES {
            let got = check(&plan(*family, url), "PROPFIND", target, Some("0"));
            assert_eq!(got.is_ok(), *allowed, "{name}");
        }
    }
}
