//! The discovery readers over recorded bodies, and the walk over scripted replies.

use dav_proto::carddav::Props;
use dav_proto::discover::{
    Error, Home, Next, Reach, Scope, Walk, collections, current_user_principal, home_set,
};
use dav_proto::parse_multistatus;
use http::{Request, Response, Uri};

fn text(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn fixture(name: &str) -> dav_proto::Multistatus {
    parse_multistatus(&text(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn nextcloud_discovery() {
    let principal = fixture("nextcloud-principal.xml");
    assert_eq!(
        current_user_principal(&principal).as_deref(),
        Some("/remote.php/dav/principals/users/alice/")
    );
    let homes = fixture("nextcloud-homes.xml");
    assert_eq!(
        home_set(&homes, Home::Calendar),
        ["/remote.php/dav/calendars/alice/"]
    );
    assert_eq!(
        home_set(&homes, Home::Addressbook),
        ["/remote.php/dav/addressbooks/users/alice/"]
    );
    let calendars = fixture("nextcloud-calendars.xml");
    assert_eq!(
        collections(&calendars, Home::Calendar),
        ["/remote.php/dav/calendars/alice/personal/"]
    );
    assert!(collections(&calendars, Home::Addressbook).is_empty());
}

#[test]
fn fastmail_discovery() {
    let principal = fixture("fastmail-principal.xml");
    assert_eq!(
        current_user_principal(&principal).as_deref(),
        Some("/dav/principals/user/alice@fastmail.example/")
    );
    assert_eq!(
        home_set(&principal, Home::Calendar),
        ["/dav/calendars/user/alice@fastmail.example/"]
    );
    assert_eq!(
        home_set(&principal, Home::Addressbook),
        ["/dav/addressbooks/user/alice@fastmail.example/"]
    );
}

#[test]
fn a_collection_is_found_beside_failed_members() {
    let ms = fixture("generic-mixed.xml");
    assert_eq!(collections(&ms, Home::Calendar), ["/dav/cal/work/"]);
}

const HOME_SETS: &str = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"
  xmlns:card="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/p/</d:href>
  <d:propstat><d:prop>
    <d:current-user-principal><d:href>/principals/ada lovelace/</d:href></d:current-user-principal>
    <c:calendar-home-set><d:href>/cal/ada lovelace/</d:href><d:href>/cal/shared/</d:href></c:calendar-home-set>
    <card:addressbook-home-set><d:href> /books/my books/ </d:href></card:addressbook-home-set>
  </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;

#[test]
fn an_href_with_a_space_in_it_is_one_href_not_two() {
    let status = parse_multistatus(HOME_SETS).unwrap();
    assert_eq!(
        current_user_principal(&status).as_deref(),
        Some("/principals/ada lovelace/")
    );
    assert_eq!(
        home_set(&status, Home::Calendar),
        ["/cal/ada lovelace/", "/cal/shared/"]
    );
    assert_eq!(home_set(&status, Home::Addressbook), ["/books/my books/"]);
    let props = Props::of(&status.responses[0]);
    assert_eq!(
        props.current_user_principal.as_deref(),
        Some("/principals/ada lovelace/")
    );
    assert_eq!(props.addressbook_home_set, ["/books/my books/"]);
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

fn reply(status: u16, body: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(body.as_bytes().to_vec())
        .unwrap()
}

/// Runs a walk against `answers` (path, status, body), returning what it found (or why it
/// stopped) and the requests it asked.
fn run(
    endpoint: &str,
    reach: Reach,
    wants: &[Home],
    scope: Scope,
    answers: &[(&str, u16, String)],
) -> (
    Result<dav_proto::discover::Found, Error>,
    Vec<Request<Vec<u8>>>,
) {
    let (mut walk, mut request) =
        Walk::start(endpoint.parse::<Uri>().unwrap(), reach, wants, scope);
    let mut asked = Vec::new();
    loop {
        let path = request.uri().to_string();
        let answer = answers
            .iter()
            .find(|(p, _, _)| path.ends_with(p))
            .unwrap_or_else(|| panic!("nothing scripted for {path}"));
        asked.push(request);
        match walk.advance(&reply(answer.1, &answer.2)) {
            Ok(Next::Ask(next)) => request = next,
            Ok(Next::Done(found)) => return (Ok(found), asked),
            Err(error) => return (Err(error), asked),
        }
    }
}

fn principal_body() -> String {
    multi(&response(
        "/dav/",
        "<d:current-user-principal><d:href>/dav/p/a/</d:href></d:current-user-principal>",
    ))
}

fn homes_body() -> String {
    multi(&response(
        "/dav/p/a/",
        "<c:calendar-home-set><d:href>/dav/cal/a/</d:href></c:calendar-home-set>\
         <card:addressbook-home-set><d:href>/dav/books/a/</d:href></card:addressbook-home-set>",
    ))
}

#[test]
fn the_walk_goes_endpoint_principal_homes() {
    let (found, asked) = run(
        "http://dav.example.test/dav/",
        Reach::Homes,
        &[Home::Calendar, Home::Addressbook],
        Scope::SameOrigin,
        &[
            ("/dav/", 207, principal_body()),
            ("/dav/p/a/", 207, homes_body()),
        ],
    );
    let found = found.unwrap();
    assert_eq!(
        found.principal.as_ref().unwrap().to_string(),
        "http://dav.example.test/dav/p/a/"
    );
    assert_eq!(
        found.homes_of(Home::Calendar),
        ["http://dav.example.test/dav/cal/a/".parse::<Uri>().unwrap()]
    );
    assert_eq!(
        found.homes_of(Home::Addressbook),
        ["http://dav.example.test/dav/books/a/"
            .parse::<Uri>()
            .unwrap()]
    );
    assert_eq!(asked.len(), 2);
    for request in &asked {
        assert_eq!(request.method(), "PROPFIND");
        assert_eq!(request.headers()["depth"], "0");
    }
    // The first asks for the principal and both home sets; the second only for the homes.
    let first = String::from_utf8(asked[0].body().clone()).unwrap();
    assert!(first.contains("current-user-principal") && first.contains("calendar-home-set"));
    let second = String::from_utf8(asked[1].body().clone()).unwrap();
    assert!(!second.contains("current-user-principal") && second.contains("addressbook-home-set"));
}

#[test]
fn a_walk_to_the_principal_asks_once() {
    let (found, asked) = run(
        "http://dav.example.test/dav/",
        Reach::Principal,
        &[],
        Scope::SameOrigin,
        &[("/dav/", 207, principal_body())],
    );
    assert_eq!(found.unwrap().principal.unwrap().path(), "/dav/p/a/");
    assert_eq!(asked.len(), 1);
    let body = String::from_utf8(asked[0].body().clone()).unwrap();
    assert!(!body.contains("home-set"), "{body}");
}

#[test]
fn the_walk_lists_the_collections_of_a_home_with_names_and_colours() {
    let cal = "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>";
    let listing = multi(
        &[
            response("/dav/cal/a/", "<d:resourcetype><d:collection/></d:resourcetype>"),
            response(
                "/dav/cal/a/personal/",
                &format!("{cal}<d:displayname>Personal</d:displayname><ic:calendar-color>#0082c9FF</ic:calendar-color>"),
            ),
            response(
                "/dav/cal/a/work stuff/",
                &format!("{cal}<d:displayname>Work</d:displayname>"),
            ),
            response(
                "/dav/cal/a/inbox/",
                "<d:resourcetype><d:collection/><c:schedule-inbox/></d:resourcetype>",
            ),
        ]
        .concat(),
    );
    let (found, _) = run(
        "http://dav.example.test/dav/",
        Reach::Collections,
        &[Home::Calendar],
        Scope::SameOrigin,
        &[
            ("/dav/", 207, principal_body()),
            ("/dav/p/a/", 207, homes_body()),
            ("/dav/cal/a/", 207, listing),
        ],
    );
    let got: Vec<_> = found
        .unwrap()
        .collections
        .into_iter()
        .map(|c| (c.url.to_string(), c.display_name, c.color))
        .collect();
    assert_eq!(
        got,
        [
            (
                "http://dav.example.test/dav/cal/a/personal/".to_owned(),
                Some("Personal".to_owned()),
                Some("#0082c9FF".to_owned())
            ),
            (
                "http://dav.example.test/dav/cal/a/work%20stuff/".to_owned(),
                Some("Work".to_owned()),
                None
            ),
        ]
    );
}

#[test]
fn a_server_without_a_principal_is_read_as_a_home_and_a_collection_as_its_own_member() {
    let plain = multi(&response(
        "/dav/",
        "<d:resourcetype><d:collection/></d:resourcetype>",
    ));
    let (found, asked) = run(
        "http://dav.example.test/dav/",
        Reach::Collections,
        &[Home::Calendar],
        Scope::SameOrigin,
        &[("/dav/", 207, plain)],
    );
    assert!(found.unwrap().collections.is_empty());
    assert_eq!(
        asked.len(),
        2,
        "the endpoint is asked for a principal, then listed as the home"
    );

    let book = multi(&response(
        "/dav/contacts/",
        "<d:resourcetype><d:collection/><card:addressbook/></d:resourcetype><d:displayname>Book</d:displayname>",
    ));
    let (found, _) = run(
        "http://dav.example.test/dav/contacts/",
        Reach::Collections,
        &[Home::Addressbook],
        Scope::SameOrigin,
        &[("/dav/contacts/", 207, book)],
    );
    let found = found.unwrap();
    assert_eq!(found.collections.len(), 1);
    assert_eq!(found.collections[0].display_name.as_deref(), Some("Book"));
}

#[test]
fn a_refusal_a_status_and_a_body_that_is_not_a_multistatus_are_told_apart() {
    let cases = [
        (401, String::new(), Error::Unauthorized),
        (403, String::new(), Error::Unauthorized),
        (404, String::new(), Error::Status(404)),
        (500, String::new(), Error::Status(500)),
        (207, "nope".to_owned(), Error::Unreadable),
        (200, "<html/>".to_owned(), Error::Unreadable),
    ];
    for (status, body, want) in cases {
        let (found, _) = run(
            "http://dav.example.test/dav/",
            Reach::Collections,
            &[Home::Calendar],
            Scope::SameOrigin,
            &[("/dav/", status, body)],
        );
        assert_eq!(found.unwrap_err(), want, "{status}");
    }
}

#[test]
fn the_principal_stays_known_when_a_later_reply_fails() {
    let (mut walk, _) = Walk::start(
        "http://dav.example.test/dav/".parse().unwrap(),
        Reach::Homes,
        &[Home::Calendar],
        Scope::SameOrigin,
    );
    assert!(matches!(
        walk.advance(&reply(207, &principal_body())),
        Ok(Next::Ask(_))
    ));
    assert_eq!(
        walk.advance(&reply(500, "")).unwrap_err(),
        Error::Status(500)
    );
    assert_eq!(walk.principal().unwrap().path(), "/dav/p/a/");
}

#[test]
fn an_href_to_another_origin_is_followed_only_where_the_scope_allows() {
    let elsewhere = multi(&response(
        "/dav/",
        "<d:current-user-principal><d:href>https://other.example.test/p/a/</d:href></d:current-user-principal>",
    ));
    let answers = [
        ("/dav/", 207, elsewhere),
        (
            "/p/a/",
            207,
            multi(&response(
                "/p/a/",
                "<c:calendar-home-set><d:href>/cal/a/</d:href></c:calendar-home-set>",
            )),
        ),
    ];
    let (found, asked) = run(
        "https://dav.example.test/dav/",
        Reach::Homes,
        &[Home::Calendar],
        Scope::SameOrigin,
        &answers,
    );
    let found = found.unwrap();
    assert_eq!(found.principal, None);
    assert!(found.homes_of(Home::Calendar).is_empty());
    assert_eq!(asked.len(), 1, "nothing is sent to the other host");

    let (found, asked) = run(
        "https://dav.example.test/dav/",
        Reach::Homes,
        &[Home::Calendar],
        Scope::Anywhere,
        &answers,
    );
    assert_eq!(
        found.unwrap().homes_of(Home::Calendar),
        ["https://other.example.test/cal/a/".parse::<Uri>().unwrap()]
    );
    assert_eq!(asked[1].uri().host(), Some("other.example.test"));
}

#[test]
fn the_default_port_is_the_same_origin() {
    let explicit = multi(&response(
        "/dav/",
        "<d:current-user-principal><d:href>https://DAV.example.test:443/p/a/</d:href></d:current-user-principal>",
    ));
    let (found, _) = run(
        "https://dav.example.test/dav/",
        Reach::Principal,
        &[],
        Scope::SameOrigin,
        &[("/dav/", 207, explicit)],
    );
    assert_eq!(found.unwrap().principal.unwrap().path(), "/p/a/");
}

#[test]
fn a_home_named_twice_is_listed_once() {
    let twice = multi(&response(
        "/dav/",
        "<c:calendar-home-set><d:href>/cal/a</d:href><d:href>/cal/a/</d:href></c:calendar-home-set>",
    ));
    let (found, asked) = run(
        "http://dav.example.test/dav/",
        Reach::Collections,
        &[Home::Calendar],
        Scope::SameOrigin,
        &[("/dav/", 207, twice), ("/cal/a/", 207, multi(""))],
    );
    assert_eq!(found.unwrap().homes_of(Home::Calendar).len(), 1);
    assert_eq!(asked.len(), 2);
}

#[test]
fn a_path_href_resolves_on_the_endpoints_origin_with_its_port_and_a_relative_one_too() {
    for (href, want) in [
        (
            "/remote.php/dav/p/ada/",
            "http://127.0.0.1:8080/remote.php/dav/p/ada/",
        ),
        ("http://127.0.0.1:8080/dav/", "http://127.0.0.1:8080/dav/"),
        ("dav/p/", "http://127.0.0.1:8080/dav/p/"),
    ] {
        let body = multi(&response(
            "/sub/",
            &format!(
                "<d:current-user-principal><d:href>{href}</d:href></d:current-user-principal>"
            ),
        ));
        let (found, _) = run(
            "http://127.0.0.1:8080/sub",
            Reach::Principal,
            &[],
            Scope::SameOrigin,
            &[("/sub", 207, body)],
        );
        assert_eq!(
            found.unwrap().principal.unwrap().to_string(),
            want,
            "{href}"
        );
    }
}
