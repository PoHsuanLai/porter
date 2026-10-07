//! The collections of an account: from the CalDAV or CardDAV endpoint to the principal, from
//! the principal to the calendar or address book home set, and the home's calendars or address
//! books with their names and colours (RFC 5397, 4791, 6352; the requests and the reading are
//! porter-dav's, the walk follows mailo `crates/mail-pim/src/dav/mod.rs`, same author,
//! MIT OR Apache-2.0).
//!
//! A server that names no principal is read as if the endpoint were the home set, and an
//! endpoint that is itself a collection is its own only member.

use super::PimKind;
use porter_core::WebUrl;
use porter_dav::{
    Depth, Home, Multistatus, collections, current_user_principal, home_set, names,
    parse_multistatus, propfind,
};
use porter_http::{Http, HttpError, HttpRequest, Method};

/// Apple's calendar colour property, which Nextcloud, iCloud and Radicale all serve.
pub const CALENDAR_COLOR: &str = "http://apple.com/ns/ical/calendar-color";

/// One calendar or address book on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its URL, with the trailing slash of a collection.
    pub url: WebUrl,
    /// The last segment of its path, decoded: what it is called in the URL (`personal`).
    pub segment: String,
    /// The name the person gave it.
    pub displayname: Option<String>,
    /// Its colour as the server wrote it.
    pub color: Option<String>,
}

/// Why the collections could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DiscoverError {
    /// The server could not be reached.
    #[error("the server could not be reached")]
    Unreachable,
    /// The server refused the account's credential.
    #[error("the server refused the account")]
    Unauthorized,
    /// The server answered with this status.
    #[error("the server answered {0}")]
    Status(u16),
    /// The answer was not a multistatus, or an href was not a URL.
    #[error("the server's answer could not be read")]
    Unreadable,
}

fn home_of(kind: PimKind) -> Home {
    match kind {
        PimKind::Calendar | PimKind::Tasks => Home::Calendar,
        PimKind::Contacts => Home::Addressbook,
    }
}

fn home_property(kind: PimKind) -> &'static str {
    match kind {
        PimKind::Calendar | PimKind::Tasks => names::CALENDAR_HOME_SET,
        PimKind::Contacts => names::ADDRESSBOOK_HOME_SET,
    }
}

/// `scheme://authority` of a URL as written.
fn origin_of(url: &WebUrl) -> &str {
    let text = url.as_str();
    let after = text.find("://").map_or(0, |at| at + 3);
    let end = text[after..]
        .find(['/', '?'])
        .map_or(text.len(), |at| after + at);
    &text[..end]
}

/// The URL an href names, against the endpoint's origin; always with a trailing slash when
/// `collection`.
fn resolve(endpoint: &WebUrl, href: &str, collection: bool) -> Result<WebUrl, DiscoverError> {
    let mut text = match href.contains("://") {
        true => href.to_owned(),
        false => format!("{}/{}", origin_of(endpoint), href.trim_start_matches('/')),
    };
    if collection && !text.ends_with('/') {
        text.push('/');
    }
    WebUrl::parse(&text).map_err(|_| DiscoverError::Unreadable)
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn segment_of(url: &WebUrl) -> String {
    let last = url
        .path()
        .split('/')
        .rfind(|part| !part.is_empty())
        .unwrap_or("collection");
    decode(last)
}

async fn ask<H: Http>(
    http: &H,
    url: WebUrl,
    depth: Depth,
    props: &[&str],
) -> Result<Multistatus, DiscoverError> {
    let request: HttpRequest = propfind(url, depth, props);
    debug_assert_eq!(request.method, Method::Propfind);
    let response = http.send(request).await.map_err(|e| match e {
        HttpError::Unreachable => DiscoverError::Unreachable,
        _ => DiscoverError::Unreadable,
    })?;
    match response.status.0 {
        207 => parse_multistatus(&String::from_utf8_lossy(&response.body))
            .map_err(|_| DiscoverError::Unreadable),
        401 | 403 => Err(DiscoverError::Unauthorized),
        other => Err(DiscoverError::Status(other)),
    }
}

/// The home sets of `kind` the endpoint leads to, as URLs; the endpoint itself when the server
/// names none.
async fn homes<H: Http>(
    http: &H,
    endpoint: &WebUrl,
    kind: PimKind,
) -> Result<Vec<WebUrl>, DiscoverError> {
    let property = home_property(kind);
    let first = ask(
        http,
        endpoint.clone(),
        Depth::Zero,
        &[names::CURRENT_USER_PRINCIPAL, property],
    )
    .await?;
    let mut hrefs = home_set(&first, home_of(kind));
    if hrefs.is_empty()
        && let Some(principal) = current_user_principal(&first)
    {
        let principal = resolve(endpoint, &principal, true)?;
        let reply = ask(http, principal, Depth::Zero, &[property]).await?;
        hrefs = home_set(&reply, home_of(kind));
    }
    let mut urls: Vec<WebUrl> = Vec::new();
    for href in &hrefs {
        let url = resolve(endpoint, href, true)?;
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    if urls.is_empty() {
        urls.push(resolve(endpoint, endpoint.path(), true)?);
    }
    Ok(urls)
}

/// Every calendar (or address book) the account has, in the server's order.
pub async fn discover<H: Http>(
    http: &H,
    endpoint: &WebUrl,
    kind: PimKind,
) -> Result<Vec<Found>, DiscoverError> {
    let mut out: Vec<Found> = Vec::new();
    for home in homes(http, endpoint, kind).await? {
        let listing = ask(
            http,
            home,
            Depth::One,
            &[names::RESOURCETYPE, names::DISPLAYNAME, CALENDAR_COLOR],
        )
        .await?;
        for href in collections(&listing, home_of(kind)) {
            let url = resolve(endpoint, &href, true)?;
            if out.iter().any(|found| found.url == url) {
                continue;
            }
            let response = listing.responses.iter().find(|r| r.href == href);
            let text = |name: &str| {
                response
                    .and_then(|r| r.found(name))
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
            };
            out.push(Found {
                segment: segment_of(&url),
                displayname: text(names::DISPLAYNAME),
                color: text(CALENDAR_COLOR).filter(|_| kind == PimKind::Calendar),
                url,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::{HttpResponse, Status};
    use std::sync::Mutex;

    /// Answers each PROPFIND from a table of `(path, body)`, remembering what was asked.
    struct Script {
        answers: Vec<(&'static str, u16, String)>,
        asked: Mutex<Vec<String>>,
    }

    impl Http for Script {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
            let path = request.url.path().to_owned();
            self.asked.lock().expect("lock").push(path.clone());
            let (_, status, body) = self
                .answers
                .iter()
                .find(|(p, _, _)| *p == path)
                .ok_or(HttpError::Unreachable)?;
            Ok(HttpResponse {
                status: Status(*status),
                headers: Vec::new(),
                body: body.clone().into_bytes(),
            })
        }
    }

    fn multi(responses: &str) -> String {
        format!(
            r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:ic="http://apple.com/ns/ical/">{responses}</d:multistatus>"#
        )
    }

    fn response(href: &str, props: &str) -> String {
        format!(
            "<d:response><d:href>{href}</d:href><d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
        )
    }

    const CAL: &str = "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>";

    fn endpoint() -> WebUrl {
        WebUrl::parse("http://127.0.0.1:9/remote.php/dav/").expect("url")
    }

    #[tokio::test]
    async fn the_walk_goes_endpoint_principal_home_collections() {
        let script = Script {
            answers: vec![
                (
                    "/remote.php/dav/",
                    207,
                    multi(&response(
                        "/remote.php/dav/",
                        "<d:current-user-principal><d:href>/remote.php/dav/principals/users/a/</d:href></d:current-user-principal>",
                    )),
                ),
                (
                    "/remote.php/dav/principals/users/a/",
                    207,
                    multi(&response(
                        "/remote.php/dav/principals/users/a/",
                        "<c:calendar-home-set><d:href>/remote.php/dav/calendars/a/</d:href></c:calendar-home-set>",
                    )),
                ),
                (
                    "/remote.php/dav/calendars/a/",
                    207,
                    multi(&[
                        response("/remote.php/dav/calendars/a/", "<d:resourcetype><d:collection/></d:resourcetype>"),
                        response(
                            "/remote.php/dav/calendars/a/personal/",
                            &format!("{CAL}<d:displayname>Personal</d:displayname><ic:calendar-color>#0082c9FF</ic:calendar-color>"),
                        ),
                        response(
                            "/remote.php/dav/calendars/a/work%20stuff/",
                            &format!("{CAL}<d:displayname>Work</d:displayname>"),
                        ),
                        response(
                            "/remote.php/dav/calendars/a/inbox/",
                            "<d:resourcetype><d:collection/><c:schedule-inbox/></d:resourcetype>",
                        ),
                    ].concat()),
                ),
            ],
            asked: Mutex::default(),
        };
        let found = discover(&script, &endpoint(), PimKind::Calendar)
            .await
            .expect("discovered");
        let got: Vec<_> = found
            .iter()
            .map(|f| {
                (
                    f.segment.as_str(),
                    f.displayname.as_deref(),
                    f.color.as_deref(),
                    f.url.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    "personal",
                    Some("Personal"),
                    Some("#0082c9FF"),
                    "http://127.0.0.1:9/remote.php/dav/calendars/a/personal/"
                ),
                (
                    "work stuff",
                    Some("Work"),
                    None,
                    "http://127.0.0.1:9/remote.php/dav/calendars/a/work%20stuff/"
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_server_without_a_principal_is_read_as_a_home_and_a_collection_as_its_own_member() {
        let home = Script {
            answers: vec![(
                "/dav/",
                207,
                multi(&response(
                    "/dav/",
                    "<d:resourcetype><d:collection/></d:resourcetype>",
                )),
            )],
            asked: Mutex::default(),
        };
        let plain = WebUrl::parse("http://127.0.0.1:9/dav/").expect("url");
        assert_eq!(
            discover(&home, &plain, PimKind::Calendar)
                .await
                .expect("ok"),
            []
        );

        let book = Script {
            answers: vec![(
                "/dav/contacts/",
                207,
                multi(&response(
                    "/dav/contacts/",
                    "<d:resourcetype><d:collection/><card:addressbook/></d:resourcetype><d:displayname>Book</d:displayname><ic:calendar-color>#fff</ic:calendar-color>",
                )),
            )],
            asked: Mutex::default(),
        };
        let own = WebUrl::parse("http://127.0.0.1:9/dav/contacts/").expect("url");
        let found = discover(&book, &own, PimKind::Contacts).await.expect("ok");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].segment, "contacts");
        assert_eq!(found[0].displayname.as_deref(), Some("Book"));
        assert_eq!(found[0].color, None, "an address book has no colour");
    }

    #[tokio::test]
    async fn refusals_and_failures_are_told_apart() {
        const CASES: &[(u16, DiscoverError)] = &[
            (401, DiscoverError::Unauthorized),
            (403, DiscoverError::Unauthorized),
            (404, DiscoverError::Status(404)),
            (500, DiscoverError::Status(500)),
        ];
        for (status, want) in CASES {
            let script = Script {
                answers: vec![("/remote.php/dav/", *status, String::new())],
                asked: Mutex::default(),
            };
            assert_eq!(
                discover(&script, &endpoint(), PimKind::Calendar)
                    .await
                    .expect_err("fails"),
                *want
            );
        }
        let unreachable = Script {
            answers: vec![],
            asked: Mutex::default(),
        };
        assert_eq!(
            discover(&unreachable, &endpoint(), PimKind::Calendar)
                .await
                .expect_err("fails"),
            DiscoverError::Unreachable
        );
        let garbage = Script {
            answers: vec![("/remote.php/dav/", 207, "nope".to_owned())],
            asked: Mutex::default(),
        };
        assert_eq!(
            discover(&garbage, &endpoint(), PimKind::Calendar)
                .await
                .expect_err("fails"),
            DiscoverError::Unreadable
        );
    }

    #[test]
    fn hrefs_resolve_against_the_endpoints_origin_and_decode() {
        let e = endpoint();
        assert_eq!(
            resolve(&e, "/a/b", true).expect("url").as_str(),
            "http://127.0.0.1:9/a/b/"
        );
        assert_eq!(
            resolve(&e, "https://other.test/x/", true)
                .expect("url")
                .as_str(),
            "https://other.test/x/"
        );
        assert_eq!(decode("a%20b%2Fc%zz"), "a b/c%zz");
    }
}
