//! The fake Nextcloud's DAV tree and the plain DAV server: PROPFIND, quota, sync tokens, PUT,
//! MKCOL, CalDAV and CardDAV.

use crate::common;

use common::{call, dav, hrefs, nextcloud};
use porter_core::Family;
use porter_fake::FakeServer;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::Request;
use porter_fake_servers::{FakeDav, shipped};

#[tokio::test]
async fn webdav_propfind_quota_and_sync_collection_agree() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    running.put_file("a.txt", b"hello");
    running.set_quota(1000, 5000);
    let root = "/remote.php/dav/files/alice/";

    let listing = call(
        &address,
        dav("PROPFIND", root, "pw").with_header("Depth", "1"),
    )
    .await;
    assert_eq!(
        hrefs(&listing),
        vec![root.to_owned(), format!("{root}a.txt")]
    );
    let text = listing.text();
    assert!(
        text.contains("<d:quota-used-bytes>1005</d:quota-used-bytes>"),
        "{text}"
    );
    assert!(text.contains("<d:quota-available-bytes>5000</d:quota-available-bytes>"));
    assert!(
        text.contains("<d:getcontentlength>5</d:getcontentlength>")
            && text.contains("<d:sync-token>")
    );

    // Only the named properties come back when the request names them.
    let narrow = call(&address, dav("PROPFIND", root, "pw").with_header("Depth", "0").with_body(
        "<d:propfind xmlns:d=\"DAV:\"><d:prop><d:quota-used-bytes/><d:quota-available-bytes/></d:prop></d:propfind>",
    ))
    .await
    .text();
    assert!(
        narrow.contains("quota-used-bytes")
            && !narrow.contains("getetag")
            && !narrow.contains("sync-token")
    );

    let report = |token: String| {
        let body = format!(
            "<d:sync-collection xmlns:d=\"DAV:\"><d:sync-token>{token}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"
        );
        dav("REPORT", root, "pw").with_body(body)
    };
    let first = call(&address, report(String::new())).await;
    assert_eq!(
        (first.status, hrefs(&first)),
        (207, vec![format!("{root}a.txt")])
    );
    let token = running.sync_token();
    assert!(first.text().contains(&token));

    // Changes since the token: a new file, and the deleted one as a 404 entry.
    running.put_file("b.txt", b"second");
    running.delete_file("a.txt");
    let delta = call(&address, report(token.clone())).await;
    assert_eq!(
        hrefs(&delta),
        vec![format!("{root}b.txt"), format!("{root}a.txt")]
    );
    assert!(delta.text().contains("404 Not Found"));
    let new_token = running.sync_token();
    assert_ne!(token, new_token);
    assert_eq!(
        hrefs(&call(&address, report(new_token)).await),
        Vec::<String>::new()
    );

    running.expire_sync_tokens();
    let expired = call(&address, report(token)).await;
    assert_eq!(expired.status, 403);
    assert!(expired.text().contains("valid-sync-token"));
}

#[tokio::test]
async fn put_mkcol_get_and_delete_work_over_the_wire() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    let root = "/remote.php/dav/files/alice";
    assert_eq!(
        call(
            &address,
            dav("PUT", &format!("{root}/x.txt"), "pw").with_body("one")
        )
        .await
        .status,
        201
    );
    assert_eq!(
        call(
            &address,
            dav("PUT", &format!("{root}/x.txt"), "pw").with_body("two")
        )
        .await
        .status,
        204
    );
    assert_eq!(
        call(
            &address,
            dav("PUT", &format!("{root}/nodir/x.txt"), "pw").with_body("z")
        )
        .await
        .status,
        409
    );
    assert_eq!(
        call(&address, dav("MKCOL", &format!("{root}/docs"), "pw"))
            .await
            .status,
        201
    );
    assert_eq!(
        call(&address, dav("MKCOL", &format!("{root}/docs"), "pw"))
            .await
            .status,
        405
    );
    assert_eq!(
        call(
            &address,
            dav("PUT", &format!("{root}/docs/y.txt"), "pw").with_body("deep")
        )
        .await
        .status,
        201
    );
    assert_eq!(
        call(&address, dav("GET", &format!("{root}/x.txt"), "pw"))
            .await
            .text(),
        "two"
    );
    assert_eq!(
        call(&address, dav("DELETE", &format!("{root}/docs"), "pw"))
            .await
            .status,
        204
    );
    assert_eq!(
        call(&address, dav("GET", &format!("{root}/docs/y.txt"), "pw"))
            .await
            .status,
        404
    );
    assert_eq!(
        call(&address, dav("PROPFIND", &format!("{root}/docs"), "pw"))
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn caldav_and_carddav_collections_list_and_sync() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    running.put_item("personal", "e1.ics", "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n");
    running.put_item("contacts", "c1.vcf", "BEGIN:VCARD\r\nEND:VCARD\r\n");

    let calendars = call(
        &address,
        dav("PROPFIND", "/remote.php/dav/calendars/alice/", "pw"),
    )
    .await;
    let text = calendars.text();
    assert!(hrefs(&calendars).contains(&"/remote.php/dav/calendars/alice/personal/".to_owned()));
    assert!(
        text.contains("<cal:calendar/>")
            && text.contains("name=\"VEVENT\"")
            && text.contains("name=\"VTODO\""),
        "{text}"
    );

    let books = call(
        &address,
        dav(
            "PROPFIND",
            "/remote.php/dav/addressbooks/users/alice/",
            "pw",
        ),
    )
    .await;
    assert!(books.text().contains("<card:addressbook/>"));

    let sync = "<d:sync-collection xmlns:d=\"DAV:\"><d:sync-token/><d:prop><d:getetag/></d:prop></d:sync-collection>";
    let events = call(
        &address,
        dav("REPORT", "/remote.php/dav/calendars/alice/personal", "pw").with_body(sync),
    )
    .await;
    assert_eq!(
        hrefs(&events),
        vec!["/remote.php/dav/calendars/alice/personal/e1.ics".to_owned()]
    );
    let tasks = call(
        &address,
        dav("REPORT", "/remote.php/dav/calendars/alice/tasks", "pw").with_body(sync),
    )
    .await;
    assert!(hrefs(&tasks).is_empty());
}

#[tokio::test]
async fn plain_dav_serves_files_behind_one_login() {
    let running = FakeDav::start("bob", "secret").await.expect("dav");
    let (address, _) = split_loopback(running.base_url()).expect("address");
    running.put_file("n.txt", b"abc");
    running.set_quota(0, 100);
    let ask = |user: &str, pw: &str| Request::new("PROPFIND", "/dav/files/").with_basic(user, pw);
    assert_eq!(call(&address, ask("bob", "bad")).await.status, 401);
    let listing = call(&address, ask("bob", "secret")).await;
    assert_eq!(
        hrefs(&listing),
        vec!["/dav/files/".to_owned(), "/dav/files/n.txt".to_owned()]
    );
    assert!(
        listing
            .text()
            .contains("<d:quota-used-bytes>3</d:quota-used-bytes>")
    );
    assert!(running.hits().iter().any(|h| h.status == 401));
}

#[tokio::test]
async fn rewrite_points_dav_rows_at_the_plain_dav_server() {
    let fake = FakeDav::bind("bob", "secret").await.expect("dav");
    let base = fake.handle().base_url().to_owned();
    let rewritten = fake.rewrite(&shipped::nextcloud());
    let webdav: Vec<_> = rewritten
        .capabilities
        .iter()
        .filter(|r| r.family == Family::WebDav)
        .collect();
    assert_eq!(
        webdav[0].endpoint.as_ref().map(|e| e.0.clone()),
        Some(format!("{base}/dav/files/"))
    );
    // Notes are Nextcloud's API, not DAV: that row is left alone.
    assert!(
        rewritten
            .capabilities
            .iter()
            .filter(|r| r.family == Family::NextcloudNotes)
            .all(|r| r.endpoint.is_none())
    );
}

const FILES: &str = "/remote.php/dav/files/alice";

#[tokio::test]
async fn put_and_delete_honour_if_match_and_if_none_match_and_answer_the_etag() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    let url = format!("{FILES}/a.txt");
    let put = |body: &str| dav("PUT", &url, "pw").with_body(body);
    let created = call(&address, put("one").with_header("If-None-Match", "*")).await;
    assert_eq!(created.status, 201);
    let etag = created.header("etag").expect("an etag").to_owned();
    let again = call(&address, put("two").with_header("If-None-Match", "*")).await;
    assert_eq!(again.status, 412);
    let stale = call(&address, put("two").with_header("If-Match", "\"0\"")).await;
    assert_eq!(stale.status, 412);
    let fresh = call(&address, put("two").with_header("If-Match", &etag)).await;
    assert_eq!(fresh.status, 204);
    assert_ne!(fresh.header("etag"), Some(etag.as_str()));
    let missing = call(
        &address,
        dav("PUT", &format!("{FILES}/none.txt"), "pw")
            .with_body("x")
            .with_header("If-Match", &etag),
    )
    .await;
    assert_eq!(missing.status, 412);
    let delete = |tag: &str| dav("DELETE", &url, "pw").with_header("If-Match", tag);
    assert_eq!(call(&address, delete(&etag)).await.status, 412);
    let now = fresh.header("etag").expect("etag").to_owned();
    assert_eq!(call(&address, delete(&now)).await.status, 204);
}

#[tokio::test]
async fn a_range_get_answers_206_with_its_span_and_416_past_the_end() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    running.put_file("n.bin", b"0123456789");
    let get = |range: &str| dav("GET", &format!("{FILES}/n.bin"), "pw").with_header("Range", range);
    let part = call(&address, get("bytes=2-4")).await;
    assert_eq!((part.status, part.text()), (206, "234".to_owned()));
    assert_eq!(part.header("content-range"), Some("bytes 2-4/10"));
    assert_eq!(call(&address, get("bytes=7-")).await.text(), "789");
    assert_eq!(call(&address, get("bytes=10-")).await.status, 416);
}

#[tokio::test]
async fn a_limit_makes_a_put_past_it_507_and_shrinks_what_quota_reports() {
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    running.set_limit(10);
    let put = |name: &str, body: &str| dav("PUT", &format!("{FILES}/{name}"), "pw").with_body(body);
    assert_eq!(call(&address, put("a", "123456")).await.status, 201);
    assert_eq!(call(&address, put("b", "12345")).await.status, 507);
    assert_eq!(call(&address, put("a", "123456789")).await.status, 204);
    let quota = call(
        &address,
        dav("PROPFIND", &format!("{FILES}/"), "pw").with_header("Depth", "0"),
    )
    .await
    .text();
    assert!(
        quota.contains("<d:quota-available-bytes>1</d:quota-available-bytes>"),
        "{quota}"
    );
}

#[tokio::test]
async fn the_report_can_be_infinite_deep_and_the_server_can_decline_it() {
    use porter_fake_servers::dav::{Behaviour, SyncCollection};
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    running.mkdir("d");
    running.put_file("d/deep.txt", b"x");
    running.put_file("top.txt", b"y");
    let report = |level: &str| {
        let body = format!(
            "<d:sync-collection xmlns:d=\"DAV:\"><d:sync-token></d:sync-token><d:sync-level>{level}</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"
        );
        dav("REPORT", &format!("{FILES}/"), "pw").with_body(body)
    };
    let shallow = hrefs(&call(&address, report("1")).await);
    let deep = hrefs(&call(&address, report("infinite")).await);
    assert_eq!(shallow.len(), 2, "{shallow:?}");
    assert_eq!(deep.len(), 3, "{deep:?}");
    running.set_behaviour(Behaviour {
        sync_collection: SyncCollection::NotImplemented,
        ..Behaviour::default()
    });
    assert_eq!(call(&address, report("1")).await.status, 501);
}

#[tokio::test]
async fn with_propagation_a_folder_etag_follows_what_is_below_it() {
    use porter_fake_servers::dav::{Behaviour, Propagation};
    let (running, address) = nextcloud().await;
    running.seed_app_password("pw");
    running.mkdir("d");
    let etag_of_root = || async {
        let body = "<d:propfind xmlns:d=\"DAV:\"><d:prop><d:getetag/></d:prop></d:propfind>";
        let reply = call(
            &address,
            dav("PROPFIND", &format!("{FILES}/"), "pw")
                .with_header("Depth", "0")
                .with_body(body),
        )
        .await;
        reply.text()
    };
    let before = etag_of_root().await;
    running.put_file("d/a", b"x");
    assert_eq!(
        etag_of_root().await,
        before,
        "off: the folder's etag is its own"
    );
    running.set_behaviour(Behaviour {
        etag_propagation: Propagation::Up,
        ..Behaviour::default()
    });
    running.put_file("d/b", b"x");
    assert_ne!(etag_of_root().await, before);
}
