//! `HyperHttp` against fake servers on loopback: the methods and bodies go through, a redirect
//! is returned and not followed, the caps and the timeout hold, and certificates are always
//! checked.
#![cfg(feature = "hyper")]

use porter_core::EndpointUrl;
use porter_core::WebUrl;
use porter_fake_servers::http::{Request, Response, serve};
use porter_fake_servers::net::{Bind, Listener, port_of};
use porter_fake_servers::tls;
use porter_http::{Http, HttpError, HttpRequest, HyperHttp, Limits, Method};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

struct Fake {
    port: u16,
    task: JoinHandle<()>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fake(secure: bool, handler: impl Fn(Request) -> Response + Send + Sync + 'static) -> Fake {
    let listener = Listener::bind(&Bind::Loopback, "http").await.expect("bind");
    let port = port_of(listener.address());
    let acceptor = secure.then(tls::acceptor);
    let task = tokio::spawn(serve(listener, acceptor, Arc::new(handler)));
    Fake { port, task }
}

fn url(scheme: &str, host: &str, port: u16, path: &str) -> WebUrl {
    WebUrl::parse(&format!("{scheme}://{host}:{port}{path}")).expect("url")
}

fn echo(request: Request) -> Response {
    let seen = format!(
        "{} {} {}|{}",
        request.method,
        request.target,
        request.header("depth").unwrap_or("-"),
        request.body_text()
    );
    Response::new(207).typed("text/plain", seen)
}

#[tokio::test]
async fn a_request_goes_out_with_its_method_headers_and_body_and_the_response_comes_back() {
    let server = fake(false, echo).await;
    let http = HyperHttp::new();
    let request = HttpRequest::new(
        Method::Propfind,
        url("http", "127.0.0.1", server.port, "/dav/"),
    )
    .with_header("Depth", "1")
    .with_body("<propfind/>");
    let response = http.send(request).await.expect("response");
    assert_eq!(response.status.0, 207);
    assert_eq!(response.header("content-type"), Some("text/plain"));
    assert_eq!(response.body, b"PROPFIND /dav/ 1|<propfind/>");
}

#[tokio::test]
async fn an_error_status_and_a_redirect_are_responses_and_a_redirect_is_not_followed() {
    let server = fake(false, |request| match request.path() {
        "/moved" => Response::new(301).with_header("Location", "/elsewhere"),
        "/missing" => Response::new(404),
        _ => Response::new(200),
    })
    .await;
    let http = HyperHttp::new();
    let get = |path: &str| {
        http.send(HttpRequest::new(
            Method::Get,
            url("http", "127.0.0.1", server.port, path),
        ))
    };
    let moved = get("/moved").await.expect("response");
    assert_eq!(
        (moved.status.0, moved.header("location")),
        (301, Some("/elsewhere"))
    );
    assert_eq!(get("/missing").await.expect("response").status.0, 404);
}

#[tokio::test]
async fn a_response_over_the_cap_is_too_large() {
    let server = fake(false, |_| {
        Response::new(200).typed("text/plain", vec![b'x'; 2048])
    })
    .await;
    let target = url("http", "127.0.0.1", server.port, "/");
    let small = HyperHttp::new().with_limits(Limits {
        max_body: 1024,
        ..Limits::default()
    });
    assert_eq!(
        small
            .send(HttpRequest::new(Method::Get, target.clone()))
            .await,
        Err(HttpError::TooLarge)
    );
    let roomy = HyperHttp::new().with_limits(Limits {
        max_body: 2048,
        ..Limits::default()
    });
    let response = roomy
        .send(HttpRequest::new(Method::Get, target))
        .await
        .expect("response");
    assert_eq!(response.body.len(), 2048);
}

#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let silent = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let port = silent.local_addr().expect("addr").port();
    let accepting = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            held.push(stream);
        }
    });
    let http = HyperHttp::new().with_limits(Limits {
        timeout: Duration::from_millis(200),
        ..Limits::default()
    });
    let outcome = http
        .send(HttpRequest::new(
            Method::Get,
            url("http", "127.0.0.1", port, "/"),
        ))
        .await;
    accepting.abort();
    assert_eq!(outcome, Err(HttpError::TimedOut));
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    let port = {
        let probe = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        probe.local_addr().expect("addr").port()
    };
    let outcome = HyperHttp::new()
        .send(HttpRequest::new(
            Method::Get,
            url("http", "127.0.0.1", port, "/"),
        ))
        .await;
    assert_eq!(outcome, Err(HttpError::Unreachable));
}

#[tokio::test]
async fn a_certificate_is_checked_against_the_roots_and_a_private_ca_must_be_added() {
    let server = fake(true, |_| Response::new(200).typed("text/plain", "secure")).await;
    let target = url("https", "localhost", server.port, "/");

    let strict = HyperHttp::new();
    assert_eq!(
        strict
            .send(HttpRequest::new(Method::Get, target.clone()))
            .await,
        Err(HttpError::Tls),
        "the scratch CA is not a platform root"
    );

    let trusting = HyperHttp::with_extra_roots(vec![tls::ca_der().to_vec()]);
    let response = trusting
        .send(HttpRequest::new(Method::Get, target))
        .await
        .expect("the added CA is trusted");
    assert_eq!(response.body, b"secure");

    let wrong_name = url("https", "127.0.0.1", server.port, "/");
    let outcome = HyperHttp::with_extra_roots(vec![tls::ca_der().to_vec()])
        .send(HttpRequest::new(Method::Get, wrong_name))
        .await;
    assert!(
        outcome.is_ok(),
        "the leaf is also valid for 127.0.0.1: {outcome:?}"
    );
}

#[tokio::test]
async fn plain_http_is_for_this_computer_only() {
    // Refused before anything is dialled, so the address need not answer.
    let plain = EndpointUrl::parse("http://192.0.2.1/").expect("url");
    assert_eq!(
        HttpRequest::to(Method::Get, &plain).map(|_| ()),
        Err(HttpError::Tls)
    );
    let other = EndpointUrl::parse("imaps://mail.example.org").expect("url");
    assert_eq!(
        HttpRequest::to(Method::Get, &other).map(|_| ()),
        Err(HttpError::Malformed)
    );
    assert!(WebUrl::parse("http://192.0.2.1/").is_err());
}

#[tokio::test]
async fn the_query_goes_out_with_the_path() {
    let server = fake(false, |request| {
        Response::new(200).typed("text/plain", request.target.clone())
    })
    .await;
    let target = url(
        "http",
        "127.0.0.1",
        server.port,
        "/mail/config?emailaddress=a%40b.test",
    );
    let response = HyperHttp::new()
        .send(HttpRequest::new(Method::Get, target))
        .await
        .expect("response");
    assert_eq!(response.body, b"/mail/config?emailaddress=a%40b.test");
}
