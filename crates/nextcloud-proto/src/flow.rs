//! Login Flow v2's two calls: start a flow, and poll it.
//!
//! `POST /index.php/login/v2` answers the page the person opens and where to poll; `POST` to the
//! poll endpoint with the token answers 404 until the person approves, then the server, the login
//! name and a new app password, once.

use crate::{Error, web};
use http::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use http::{Method, Request, Response, StatusCode};
use porter_core::{EndpointUrl, LoginName, SecretText};
use porter_ids::WebUrl;
use serde_json::Value;

/// What the server named for a flow just started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The page the person opens.
    pub login: WebUrl,
    /// Where to poll.
    pub poll: EndpointUrl,
    /// What to post there. Whoever holds it can collect the app password once the person
    /// approves, so it prints as `<redacted>` and is wiped when dropped.
    pub token: SecretText,
}

/// What the person approved: the server, who they are there, and the app password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted {
    /// The server, when the answer names one.
    pub server: Option<EndpointUrl>,
    /// The login name.
    pub login: LoginName,
    /// The app password.
    pub password: SecretText,
}

/// What one poll came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Polled {
    /// The person has not approved yet.
    Waiting,
    /// The server could not answer this time; the flow may still be alive.
    Hiccup,
    /// Approved.
    Granted(Granted),
    /// The flow is over without an app password.
    Failed,
}

/// The request that starts a flow at `server`.
pub fn start_request(server: &EndpointUrl) -> Result<Request<Vec<u8>>, Error> {
    let url = web(&crate::paths::login_flow(server).ok_or(Error::Unreadable)?)?;
    Request::builder()
        .method(Method::POST)
        .uri(url.as_str())
        .header(USER_AGENT, "Porter")
        .header(ACCEPT, "application/json")
        .body(Vec::new())
        .map_err(|_| Error::Unreadable)
}

/// What the server named for the flow. A server without Login Flow v2 (before Nextcloud 16), or
/// not a Nextcloud at all, is `Unreadable`.
pub fn started(response: &Response<Vec<u8>>) -> Result<Started, Error> {
    match response.status() {
        StatusCode::OK => started_body(response.body()).ok_or(Error::Unreadable),
        _ => Err(Error::Unreadable),
    }
}

fn started_body(body: &[u8]) -> Option<Started> {
    let json: Value = serde_json::from_slice(body).ok()?;
    let text = |pointer: &str| json.pointer(pointer).and_then(Value::as_str);
    Some(Started {
        login: WebUrl::parse(text("/login")?).ok()?,
        poll: EndpointUrl::typed_server(text("/poll/endpoint")?)?,
        token: SecretText::new(text("/poll/token").filter(|t| !t.is_empty())?),
    })
}

/// The poll's `POST`, the token as its form body.
pub fn poll_request(started: &Started) -> Result<Request<Vec<u8>>, Error> {
    let url = web(&started.poll)?;
    Request::builder()
        .method(Method::POST)
        .uri(url.as_str())
        .header(USER_AGENT, "Porter")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!("token={}", started.token.expose()).into_bytes())
        .map_err(|_| Error::Unreadable)
}

/// What one poll's `response` came to.
pub fn polled(response: &Response<Vec<u8>>) -> Polled {
    match response.status().as_u16() {
        404 => Polled::Waiting,
        200 => granted(response.body()).map_or(Polled::Failed, Polled::Granted),
        500..=599 | 429 => Polled::Hiccup,
        _ => Polled::Failed,
    }
}

fn granted(body: &[u8]) -> Option<Granted> {
    let json: Value = serde_json::from_slice(body).ok()?;
    let text = |key: &str| {
        json.get(key)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
    };
    Some(Granted {
        server: text("server").and_then(EndpointUrl::typed_server),
        login: LoginName(text("loginName")?.to_owned()),
        password: SecretText::new(text("appPassword")?),
    })
}

#[cfg(test)]
mod tests;
