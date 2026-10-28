//! The collections of an account: from the CalDAV or CardDAV endpoint to the principal, from
//! the principal to the calendar or address book home set, and the home's calendars or address
//! books with their names and colours (RFC 5397, 4791, 6352). The walk is dav-proto's
//! [`Walk`]; this sends its requests and reads what it found.
//!
//! A server that names no principal is read as if the endpoint were the home set, and an
//! endpoint that is itself a collection is its own only member.

use super::PimKind;
use dav_proto::discover::{Home, Next, Reach, Scope, Walk};
use porter_http::{Http, HttpError};
use porter_ids::WebUrl;

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

fn segment_of(url: &WebUrl) -> String {
    let last = url
        .path()
        .split('/')
        .rfind(|part| !part.is_empty())
        .unwrap_or("collection");
    porter_ids::percent::decode(last)
}

/// Every calendar (or address book) the account has, in the server's order.
pub async fn discover<H: Http>(
    http: &H,
    endpoint: &WebUrl,
    kind: PimKind,
) -> Result<Vec<Found>, DiscoverError> {
    let sending = |e| match e {
        HttpError::Unreachable | HttpError::TimedOut => DiscoverError::Unreachable,
        _ => DiscoverError::Unreadable,
    };
    let uri = porter_http::uri(endpoint).map_err(sending)?;
    let (mut walk, mut request) =
        Walk::start(uri, Reach::Collections, &[home_of(kind)], Scope::Anywhere);
    let found = loop {
        let response = http.send(request).await.map_err(sending)?;
        match walk.advance(&response) {
            Ok(Next::Ask(next)) => request = next,
            Ok(Next::Done(found)) => break found,
            Err(dav_proto::discover::Error::Unauthorized) => {
                return Err(DiscoverError::Unauthorized);
            }
            Err(dav_proto::discover::Error::Status(status)) => {
                return Err(DiscoverError::Status(status));
            }
            Err(_) => return Err(DiscoverError::Unreadable),
        }
    };
    found
        .collections
        .into_iter()
        .map(|collection| {
            let url = WebUrl::parse(&collection.url.to_string())
                .map_err(|_| DiscoverError::Unreadable)?;
            Ok(Found {
                segment: segment_of(&url),
                displayname: collection.display_name,
                color: collection.color.filter(|_| kind == PimKind::Calendar),
                url,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::http::{Request, Response};
    use std::sync::Mutex;

    /// Answers each PROPFIND from a table of `(path, body)`, remembering what was asked.
    struct Script {
        answers: Vec<(&'static str, u16, String)>,
        asked: Mutex<Vec<String>>,
    }

    impl Http for Script {
        async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
            let path = request.uri().path().to_owned();
            self.asked.lock().expect("lock").push(path.clone());
            let (_, status, body) = self
                .answers
                .iter()
                .find(|(p, _, _)| *p == path)
                .ok_or(HttpError::Unreachable)?;
            Ok(Response::builder()
                .status(*status)
                .body(body.clone().into_bytes())
                .expect("response"))
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
}
