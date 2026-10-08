//! The HTTP relay against the fake DAV server (plain, loopback) and a recording TLS server.

use crate::common;

use common::*;
use porter_core::{Family, Tls};
use porter_fake_servers::http::{Request, Response, serve};
use porter_fake_servers::net::{Bind, Listener};
use porter_fake_servers::{FakeDav, Seen, tls};
use porter_proxy::{RelayEnd, RelayFault};
use std::sync::Arc;

fn dav_plan(base: &str, auth: porter_core::RelayAuth) -> porter_core::RelayPlan {
    let mut plan = plan(
        Family::WebDav,
        &format!("{base}/dav/files/"),
        Tls::Plain,
        auth,
    );
    plan.endpoint.login.0 = "alice".into();
    plan
}

/// Reads one response: its status line and its body (by `Content-Length`).
async fn response(app: &mut App) -> (String, String) {
    let head = app.read_until("\r\n\r\n").await;
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_owned)
        })
        .map_or(0, |n| n.trim().parse::<usize>().expect("length"));
    let body = app.read_bytes(length).await;
    (head.lines().next().unwrap_or_default().to_owned(), body)
}

#[tokio::test]
async fn a_propfind_reaches_the_server_with_the_relays_authorization() {
    let dav = FakeDav::start("alice", PASSWORD).await.expect("dav");
    let base = dav.base_url().to_owned();
    let mut app = start(dav_plan(&base, password()), trusting_fakes());
    let host = base.trim_start_matches("http://").to_owned();
    app.send(&format!(
        "PROPFIND /dav/files/ HTTP/1.1\r\nHost: {host}\r\nDepth: 0\r\nContent-Length: 0\r\n\r\n"
    ))
    .await;
    let head = app.read_until("\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 207"), "{head}");
    assert!(!app.everything().contains(PASSWORD));
    let hits = dav.hits();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].method, "PROPFIND");
    assert!(
        hits[0]
            .authorization
            .as_deref()
            .is_some_and(|a| a.starts_with("Basic "))
    );
    drop(app);
}

#[tokio::test]
async fn an_authorization_the_app_sends_never_reaches_the_server() {
    let seen: Seen<Request> = Seen::default();
    let recorded = seen.clone();
    let listener = Listener::bind(&Bind::Loopback, "http").await.expect("bind");
    let port = port(listener.address());
    let task = tokio::spawn(serve(
        listener,
        Some(tls::acceptor()),
        Arc::new(move |request: Request| {
            recorded.push(request);
            Response::new(200).typed("text/plain", "ok")
        }),
    ));
    let mut plan = plan(
        Family::WebDav,
        &format!("https://127.0.0.1:{port}/dav/"),
        Tls::Implicit,
        token(),
    );
    plan.endpoint.login.0 = "alice".into();
    let mut app = start(plan, trusting_fakes());
    app.send(&format!(
        "GET /dav/x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Basic c3RvbGVuOnN0b2xlbg==\r\nauthorization: Bearer stolen\r\nProxy-Authorization: Basic eA==\r\n\r\n"
    ))
    .await;
    let (status, body) = response(&mut app).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, "ok");
    let requests = seen.all();
    assert_eq!(requests.len(), 1);
    let headers: Vec<_> = requests[0]
        .headers
        .iter()
        .filter(|(n, _)| {
            n.eq_ignore_ascii_case("authorization") || n.eq_ignore_ascii_case("proxy-authorization")
        })
        .collect();
    assert_eq!(headers.len(), 1, "{headers:?}");
    assert_eq!(
        requests[0].bearer(),
        Some(TOKEN),
        "the relay's bearer, not the app's"
    );
    assert!(!app.everything().contains(TOKEN));
    task.abort();
}

#[tokio::test]
async fn a_different_origin_is_refused_with_a_4xx_and_nothing_is_sent() {
    const CASES: &[(&str, &str)] = &[
        (
            "absolute uri on another host",
            "GET http://evil.example/x HTTP/1.1\r\nHost: {host}\r\n\r\n",
        ),
        (
            "host header of another origin",
            "GET /x HTTP/1.1\r\nHost: evil.example\r\n\r\n",
        ),
        (
            "same host, another port",
            "GET /x HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
        ),
        (
            "userinfo in the target",
            "GET http://alice@{host}/x HTTP/1.1\r\nHost: {host}\r\n\r\n",
        ),
    ];
    for (name, request) in CASES {
        let dav = FakeDav::start("alice", PASSWORD).await.expect("dav");
        let base = dav.base_url().to_owned();
        let host = base.trim_start_matches("http://").to_owned();
        let mut app = start(dav_plan(&base, password()), trusting_fakes());
        app.send(&request.replace("{host}", &host)).await;
        let text = app.read_to_end().await;
        assert!(text.starts_with("HTTP/1.1 403"), "{name}: {text}");
        assert_eq!(
            app.ended().await,
            RelayEnd::Failed(RelayFault::ForeignOrigin),
            "{name}"
        );
        assert!(dav.hits().is_empty(), "{name}: nothing reached the server");
    }
}

#[tokio::test]
async fn a_contacts_relay_reaches_the_address_books_and_not_the_files_or_ocs() {
    let contacts_plan = |base: &str| {
        let mut plan = plan(
            Family::CardDav,
            &format!("{base}/dav/addressbooks/users/alice/"),
            Tls::Plain,
            password(),
        );
        plan.endpoint.login.0 = "alice".into();
        plan.kind = porter_core::CapabilityKind::Contacts;
        plan
    };
    const REFUSED: &[(&str, &str)] = &[
        (
            "the files",
            "PROPFIND /dav/files/ HTTP/1.1\r\nHost: {host}\r\nDepth: 1\r\n\r\n",
        ),
        (
            "ocs",
            "GET /ocs/v2.php/core/apppassword HTTP/1.1\r\nHost: {host}\r\n\r\n",
        ),
        (
            "out by dot segments",
            "GET /dav/addressbooks/users/alice/../../../files/ HTTP/1.1\r\nHost: {host}\r\n\r\n",
        ),
    ];
    for (name, request) in REFUSED {
        let dav = FakeDav::start("alice", PASSWORD).await.expect("dav");
        let base = dav.base_url().to_owned();
        let host = base.trim_start_matches("http://").to_owned();
        let mut app = start(contacts_plan(&base), trusting_fakes());
        app.send(&request.replace("{host}", &host)).await;
        let text = app.read_to_end().await;
        assert!(text.starts_with("HTTP/1.1 403"), "{name}: {text}");
        assert_eq!(
            app.ended().await,
            RelayEnd::Failed(RelayFault::ForeignOrigin),
            "{name}"
        );
        assert!(dav.hits().is_empty(), "{name}: nothing reached the server");
    }
    let dav = FakeDav::start("alice", PASSWORD).await.expect("dav");
    let base = dav.base_url().to_owned();
    let host = base.trim_start_matches("http://").to_owned();
    let mut app = start(contacts_plan(&base), trusting_fakes());
    app.send(&format!(
        "PROPFIND /dav/addressbooks/users/alice/ HTTP/1.1\r\nHost: {host}\r\nDepth: 1\r\nContent-Length: 0\r\n\r\n"
    ))
    .await;
    let head = app.read_until("\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 "), "{head}");
    assert!(!head.starts_with("HTTP/1.1 403"), "{head}");
    let hits = dav.hits();
    assert_eq!(hits.len(), 1, "the address book request reached the server");
    assert_eq!(hits[0].method, "PROPFIND");
    drop(app);
}

#[tokio::test]
async fn keep_alive_requests_with_length_and_chunked_bodies_pass() {
    let dav = FakeDav::start("alice", PASSWORD).await.expect("dav");
    let base = dav.base_url().to_owned();
    let host = base.trim_start_matches("http://").to_owned();
    let mut app = start(dav_plan(&base, password()), trusting_fakes());

    app.send(&format!(
        "PUT /dav/files/a.txt HTTP/1.1\r\nHost: {host}\r\nContent-Length: 5\r\n\r\nhello"
    ))
    .await;
    let (status, _) = response(&mut app).await;
    assert!(
        status.starts_with("HTTP/1.1 201") || status.starts_with("HTTP/1.1 204"),
        "{status}"
    );

    // The body's own bytes look like a request: they must stay body.
    let tricky = "GET /evil HTTP/1.1\r\nHost: evil.example\r\n\r\n";
    app.send(&format!(
        "PUT /dav/files/b.txt HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{tricky}",
        tricky.len()
    ))
    .await;
    let (status, _) = response(&mut app).await;
    assert!(
        status.starts_with("HTTP/1.1 201") || status.starts_with("HTTP/1.1 204"),
        "{status}"
    );

    app.send(&format!(
        "GET /dav/files/a.txt HTTP/1.1\r\nHost: {host}\r\n\r\nGET /dav/files/b.txt HTTP/1.1\r\nHost: {host}\r\n\r\n"
    ))
    .await;
    let (status, body) = response(&mut app).await;
    assert_eq!(
        (status.as_str(), body.as_str()),
        ("HTTP/1.1 200 OK", "hello")
    );
    let (_, body) = response(&mut app).await;
    assert_eq!(body, tricky);

    let hits = dav.hits();
    assert_eq!(
        hits.iter()
            .map(|h| (h.method.as_str(), h.status))
            .collect::<Vec<_>>(),
        [("PUT", 201), ("PUT", 201), ("GET", 200), ("GET", 200)]
    );
    assert!(!app.everything().contains(PASSWORD));
    drop(app);
}

#[tokio::test]
async fn a_chunked_upload_reaches_the_server_whole() {
    // The fake HTTP server reads only `Content-Length` bodies, so this server is raw: it keeps
    // every byte it is sent and answers each request head it sees.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let captured = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = Arc::clone(&captured);
    let task = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut conn, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 1024];
        loop {
            let n = conn.read(&mut buf).await.expect("read");
            if n == 0 {
                break;
            }
            let all = {
                let mut all = sink.lock().unwrap();
                all.extend_from_slice(&buf[..n]);
                String::from_utf8_lossy(&all).into_owned()
            };
            if all.contains("GET /g HTTP/1.1") && all.ends_with("\r\n\r\n") {
                conn.write_all(b"HTTP/1.1 204 No Content\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n")
                    .await
                    .expect("write");
            }
        }
    });
    let mut plan = plan(
        Family::WebDav,
        &format!("http://127.0.0.1:{port}/"),
        Tls::Plain,
        password(),
    );
    plan.endpoint.login.0 = "alice".into();
    let mut app = start(plan, trusting_fakes());
    let request = format!(
        "PUT /f HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"
    );
    // Split mid-chunk to see the machine carry its place.
    let (first, second) = request.split_at(request.len() - 9);
    app.send(first).await;
    app.send(second).await;
    app.send(&format!(
        "GET /g HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
    ))
    .await;
    app.read_until("\r\n\r\n").await;
    app.read_until("\r\n\r\n").await;
    let seen = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
    assert!(
        seen.contains("3\r\nabc\r\n2\r\nde\r\n0\r\n\r\nGET /g HTTP/1.1"),
        "{seen:?}"
    );
    assert_eq!(seen.matches("Authorization: Basic ").count(), 2, "{seen:?}");
    task.abort();
}
