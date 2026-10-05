//! Login Flow v2's two calls: start a flow, and poll it.
//!
//! `POST /index.php/login/v2` answers the page the person opens and where to poll; `POST` to the
//! poll endpoint with the token answers 404 until the person approves, then the server, the login
//! name and a new app password, once.

use crate::io::Io;
use crate::password::{fault_of, join, parse_server};
use porter_core::sheet::SignInFault;
use porter_core::{EndpointUrl, LoginName, SecretText};
use porter_http::{Http, HttpRequest, Method, Status};
use serde_json::Value;

/// What the server named for a flow just started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Started {
    /// The page the person opens.
    pub(super) login: EndpointUrl,
    /// Where to poll.
    pub(super) poll: EndpointUrl,
    /// What to post there.
    pub(super) token: String,
}

/// What the person approved: the server, who they are there, and the app password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Granted {
    pub(super) server: Option<EndpointUrl>,
    pub(super) login: LoginName,
    pub(super) password: SecretText,
}

/// What one poll came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Polled {
    /// The person has not approved yet.
    Waiting,
    /// The server could not answer this time; the flow may still be alive.
    Hiccup,
    /// Approved.
    Granted(Granted),
    /// The flow is over without an app password.
    Failed(SignInFault),
}

/// Starts a flow at `server`.
pub(super) async fn start(io: &Io, server: &EndpointUrl) -> Result<Started, SignInFault> {
    let url = join(server, "/index.php/login/v2").ok_or(SignInFault::Unreadable)?;
    let request = HttpRequest::new(Method::Post, url)
        .with_header("User-Agent", "Porter")
        .with_header("Accept", "application/json");
    let response = io.http.send(request).await.map_err(fault_of)?;
    match response.status {
        Status(200) => started(&response.body).ok_or(SignInFault::Unreadable),
        // A server without Login Flow v2 (before Nextcloud 16) or not a Nextcloud at all.
        _ => Err(SignInFault::Unreadable),
    }
}

fn started(body: &[u8]) -> Option<Started> {
    let json: Value = serde_json::from_slice(body).ok()?;
    let text = |pointer: &str| json.pointer(pointer).and_then(Value::as_str);
    Some(Started {
        login: parse_server(text("/login")?)?,
        poll: parse_server(text("/poll/endpoint")?)?,
        token: text("/poll/token").filter(|t| !t.is_empty())?.to_owned(),
    })
}

/// Polls once.
pub(super) async fn poll(io: &Io, started: &Started) -> Polled {
    let request = HttpRequest::new(Method::Post, started.poll.clone())
        .with_header("User-Agent", "Porter")
        .with_header("Content-Type", "application/x-www-form-urlencoded")
        .with_body(format!("token={}", started.token));
    let response = match io.http.send(request).await {
        Ok(response) => response,
        Err(error) => {
            return match fault_of(error) {
                SignInFault::Unreachable => Polled::Hiccup,
                fault => Polled::Failed(fault),
            };
        }
    };
    match response.status.0 {
        404 => Polled::Waiting,
        200 => {
            granted(&response.body).map_or(Polled::Failed(SignInFault::Unreadable), Polled::Granted)
        }
        500..=599 | 429 => Polled::Hiccup,
        _ => Polled::Failed(SignInFault::Unreadable),
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
        server: text("server").and_then(parse_server),
        login: LoginName(text("loginName")?.to_owned()),
        password: SecretText::new(text("appPassword")?),
    })
}

#[cfg(test)]
mod tests;
