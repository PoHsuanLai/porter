//! The fake Nextcloud driven with a minimal HTTP client: login, OCS, capabilities, notes.

mod common;

use common::{call, dav, nextcloud};
use porter_core::Family;
use porter_fake::{FakeAddress, FakeServer};
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, post_form};
use porter_fake_servers::{FakeNextcloud, LoginPolicy, NextcloudHandle, shipped};

/// Signs in through Login Flow v2 with the scripted approval; returns the app password.
async fn sign_in(running: &NextcloudHandle, address: &FakeAddress) -> String {
    running.set_login_policy(LoginPolicy::Approve);
    let started = call(
        address,
        Request::new("POST", "/index.php/login/v2").with_header("User-Agent", "porter test"),
    )
    .await;
    let flow = started.json_body().expect("json");
    let poll_token = flow["poll"]["token"].as_str().expect("token").to_owned();
    let login = flow["login"].as_str().expect("login url").to_owned();
    // Before the person opens the page the poll finds nothing.
    assert_eq!(
        post_form(
            address,
            "/index.php/login/v2/poll",
            &[("token", &poll_token)]
        )
        .await
        .expect("poll")
        .status,
        404
    );
    let (_, target) = split_loopback(&login).expect("login url");
    assert_eq!(
        call(address, Request::new("GET", &target)).await.status,
        200
    );
    let done = post_form(
        address,
        "/index.php/login/v2/poll",
        &[("token", &poll_token)],
    )
    .await
    .expect("poll");
    assert_eq!(done.status, 200);
    let body = done.json_body().expect("json");
    assert_eq!(
        (body["loginName"].as_str(), body["server"].as_str()),
        (Some("alice"), Some(running.base_url()))
    );
    // The flow is spent.
    assert_eq!(
        post_form(
            address,
            "/index.php/login/v2/poll",
            &[("token", &poll_token)]
        )
        .await
        .expect("poll")
        .status,
        404
    );
    body["appPassword"]
        .as_str()
        .expect("app password")
        .to_owned()
}

#[tokio::test]
async fn login_flow_v2_hands_out_an_app_password_that_the_ocs_delete_revokes() {
    let (running, address) = nextcloud().await;
    let password = sign_in(&running, &address).await;
    assert_eq!(running.app_passwords(), vec![password.clone()]);

    assert_eq!(
        call(
            &address,
            Request::new("PROPFIND", "/remote.php/dav/files/alice/")
        )
        .await
        .status,
        401
    );
    assert_eq!(
        call(
            &address,
            dav("PROPFIND", "/remote.php/dav/files/alice/", "wrong")
        )
        .await
        .status,
        401
    );
    assert_eq!(
        call(
            &address,
            dav("PROPFIND", "/remote.php/dav/files/alice/", &password)
        )
        .await
        .status,
        207
    );

    let deleted = call(
        &address,
        dav("DELETE", "/ocs/v2.php/core/apppassword", &password)
            .with_header("OCS-APIRequest", "true"),
    )
    .await;
    assert_eq!(
        deleted.json_body().expect("json")["ocs"]["meta"]["statuscode"],
        200
    );
    assert!(running.app_passwords().is_empty());
    assert_eq!(
        call(
            &address,
            dav("PROPFIND", "/remote.php/dav/files/alice/", &password)
        )
        .await
        .status,
        401
    );

    // Every request was recorded with the Authorization header as received.
    let hits = running.hits();
    assert!(hits.iter().any(|h| {
        h.method == "DELETE"
            && h.authorization
                .as_deref()
                .is_some_and(|a| a.starts_with("Basic "))
    }));
    assert!(
        hits.iter()
            .any(|h| h.target.starts_with("/index.php/login/v2/flow/"))
    );
}

#[tokio::test]
async fn a_pending_or_denied_login_never_yields_a_password() {
    let (running, address) = nextcloud().await;
    let flow = call(&address, Request::new("POST", "/index.php/login/v2"))
        .await
        .json_body()
        .expect("json");
    let token = flow["poll"]["token"].as_str().expect("token").to_owned();
    let (_, target) = split_loopback(flow["login"].as_str().expect("login")).expect("url");
    // Pending: opening the page changes nothing until the test approves by token.
    call(&address, Request::new("GET", &target)).await;
    assert_eq!(
        post_form(&address, "/index.php/login/v2/poll", &[("token", &token)])
            .await
            .expect("poll")
            .status,
        404
    );
    running.approve_login(&token);
    assert_eq!(
        post_form(&address, "/index.php/login/v2/poll", &[("token", &token)])
            .await
            .expect("poll")
            .status,
        200
    );

    running.set_login_policy(LoginPolicy::Deny);
    let flow = call(&address, Request::new("POST", "/index.php/login/v2"))
        .await
        .json_body()
        .expect("json");
    let (_, target) = split_loopback(flow["login"].as_str().expect("login")).expect("url");
    call(&address, Request::new("GET", &target)).await;
    let poll = post_form(
        &address,
        "/index.php/login/v2/poll",
        &[("token", flow["poll"]["token"].as_str().expect("token"))],
    )
    .await
    .expect("poll");
    assert_eq!(poll.status, 404);
}

#[tokio::test]
async fn status_and_capabilities_need_no_login() {
    let (_running, address) = nextcloud().await;
    let status = call(&address, Request::new("GET", "/status.php"))
        .await
        .json_body()
        .expect("json");
    assert_eq!(status["installed"], true);
    let caps = call(
        &address,
        Request::new("GET", "/ocs/v2.php/cloud/capabilities?format=json"),
    )
    .await
    .json_body()
    .expect("json");
    assert!(caps["ocs"]["data"]["capabilities"]["notes"].is_object());
}

#[tokio::test]
async fn notes_api_lists_with_an_etag_and_round_trips() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    let notes = "/index.php/apps/notes/api/v1/notes";
    let json_req = |method: &str, path: &str, body: &str| {
        dav(method, path, "pw")
            .with_header("Content-Type", "application/json")
            .with_body(body)
    };

    let empty = call(&address, dav("GET", notes, "pw")).await;
    assert_eq!(empty.json_body().expect("json"), serde_json::json!([]));
    let created = call(
        &address,
        json_req(
            "POST",
            notes,
            r##"{"content":"# Shopping\nmilk","category":"home"}"##,
        ),
    )
    .await
    .json_body()
    .expect("json");
    let id = created["id"].as_u64().expect("id");
    assert_eq!(
        (created["title"].as_str(), created["category"].as_str()),
        (Some("Shopping"), Some("home"))
    );

    let list = call(&address, dav("GET", notes, "pw")).await;
    let etag = list.header("etag").expect("etag").to_owned();
    assert_eq!(
        list.json_body().expect("json").as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        call(
            &address,
            dav("GET", notes, "pw").with_header("If-None-Match", &etag)
        )
        .await
        .status,
        304
    );

    let updated = call(
        &address,
        json_req(
            "PUT",
            &format!("{notes}/{id}"),
            r##"{"content":"# Shopping\nmilk, eggs"}"##,
        ),
    )
    .await
    .json_body()
    .expect("json");
    assert_eq!(updated["category"], "home");
    assert_eq!(
        call(
            &address,
            dav("GET", notes, "pw").with_header("If-None-Match", &etag)
        )
        .await
        .status,
        200
    );
    assert_eq!(
        call(&address, dav("DELETE", &format!("{notes}/{id}"), "pw"))
            .await
            .status,
        200
    );
    assert_eq!(
        call(&address, dav("GET", &format!("{notes}/{id}"), "pw"))
            .await
            .status,
        404
    );
    assert_eq!(call(&address, Request::new("GET", notes)).await.status, 401);
}

#[tokio::test]
async fn rewrite_points_the_shipped_nextcloud_rows_at_the_fake() {
    let fake = FakeNextcloud::bind("alice").await.expect("nextcloud");
    let base = fake.handle().base_url().to_owned();
    let spec = shipped::nextcloud();
    assert!(spec.capabilities.iter().all(|r| r.endpoint.is_none()));
    let rewritten = fake.rewrite(&spec);
    let endpoint = |family: Family| {
        rewritten
            .capabilities
            .iter()
            .filter(|r| r.family == family)
            .map(|r| r.endpoint.as_ref().expect("rewritten").0.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        endpoint(Family::WebDav),
        vec![format!("{base}/remote.php/dav/files/alice/")]
    );
    assert_eq!(
        endpoint(Family::CalDav),
        vec![format!("{base}/remote.php/dav/"); 2]
    );
    assert_eq!(
        endpoint(Family::NextcloudNotes),
        vec![format!("{base}/index.php/apps/notes/api/v1/")]
    );
    assert_eq!(rewritten.capabilities.len(), spec.capabilities.len());
}

/// A PROPFIND at depth `depth` asking for `props` (each an element of its own namespace).
fn propfind(path: &str, depth: &str, props: &str, password: &str) -> Request {
    let body = format!(
        r#"<d:propfind xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:ic="http://apple.com/ns/ical/"><d:prop>{props}</d:prop></d:propfind>"#
    );
    dav("PROPFIND", path, password)
        .with_header("Depth", depth)
        .with_body(body.into_bytes())
}

#[tokio::test]
async fn discovery_walks_principal_home_sets_and_collections_with_names_and_colours() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    running.add_calendar("work", "VEVENT", "Work & Play", "#ff0000FF");
    running.add_addressbook("friends", "Friends");
    running.set_collection_meta("personal", "Personal", Some("#0082c9FF"));

    // The endpoint names the principal.
    let root = call(
        &address,
        propfind("/remote.php/dav/", "0", "<d:current-user-principal/>", "pw"),
    )
    .await;
    assert_eq!(root.status, 207);
    assert_eq!(
        common::hrefs(&root).get(1).map(String::as_str),
        Some("/remote.php/dav/principals/users/alice/")
    );
    // The principal names both home sets.
    let principal = call(
        &address,
        propfind(
            "/remote.php/dav/principals/users/alice/",
            "0",
            "<cal:calendar-home-set/><card:addressbook-home-set/>",
            "pw",
        ),
    )
    .await;
    let homes = common::hrefs(&principal);
    assert!(
        homes.contains(&"/remote.php/dav/calendars/alice/".to_owned()),
        "{homes:?}"
    );
    assert!(
        homes.contains(&"/remote.php/dav/addressbooks/users/alice/".to_owned()),
        "{homes:?}"
    );
    // The calendar home lists its calendars with their names and colours.
    let listing = call(
        &address,
        propfind(
            "/remote.php/dav/calendars/alice/",
            "1",
            "<d:resourcetype/><d:displayname/><ic:calendar-color/>",
            "pw",
        ),
    )
    .await;
    let text = listing.text();
    assert!(
        text.contains("<d:displayname>Work &amp; Play</d:displayname>"),
        "{text}"
    );
    assert!(
        text.contains("<ic:calendar-color>#ff0000FF</ic:calendar-color>"),
        "{text}"
    );
    assert!(
        text.contains("<d:displayname>Personal</d:displayname>"),
        "{text}"
    );
    assert!(
        text.contains("<ic:calendar-color>#0082c9FF</ic:calendar-color>"),
        "{text}"
    );
    // The address book home lists its books, and a book has no colour.
    let books = call(
        &address,
        propfind(
            "/remote.php/dav/addressbooks/users/alice/",
            "1",
            "<d:resourcetype/><d:displayname/><ic:calendar-color/>",
            "pw",
        ),
    )
    .await;
    let text = books.text();
    assert!(
        text.contains("<d:displayname>Friends</d:displayname>"),
        "{text}"
    );
    assert!(!text.contains("calendar-color>"), "{text}");
    assert!(text.contains("<card:addressbook/>"), "{text}");

    // Items and collections come and go.
    running.put_item("work", "w.ics", "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n");
    running.delete_item("work", "w.ics");
    running.delete_collection("work");
    let gone = call(
        &address,
        propfind(
            "/remote.php/dav/calendars/alice/work/",
            "0",
            "<d:resourcetype/>",
            "pw",
        ),
    )
    .await;
    assert_eq!(gone.status, 404);
}
