//! Helpers the Nextcloud and DAV self-tests share.

#![allow(dead_code)]

use porter_fake::FakeAddress;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Response, Scheme, send};
use porter_fake_servers::{FakeNextcloud, NextcloudHandle, Running};

pub async fn nextcloud() -> (Running<NextcloudHandle>, FakeAddress) {
    let running = FakeNextcloud::start("alice").await.expect("nextcloud");
    let (address, _) = split_loopback(running.base_url()).expect("address");
    (running, address)
}

pub async fn call(address: &FakeAddress, request: Request) -> Response {
    send(address, Scheme::Http, &request)
        .await
        .expect("request")
}

pub fn dav(method: &str, path: &str, password: &str) -> Request {
    Request::new(method, path).with_basic("alice", password)
}

pub fn hrefs(response: &Response) -> Vec<String> {
    let text = response.text();
    text.split("<d:href>")
        .skip(1)
        .filter_map(|s| s.split("</d:href>").next())
        .map(str::to_owned)
        .collect()
}
