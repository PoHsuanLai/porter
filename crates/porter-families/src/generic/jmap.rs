//! A JMAP server a person typed by hand: the session resource at the URL they gave, fetched
//! with HTTP Basic (the login name and the password) or, for an API token, a bearer, read the
//! way discovery reads a session it found. A refusal is the credential's; what is not a
//! session is unreadable. The relay presents the stored secret the same way: a password as
//! Basic, a token as a bearer (`porter-service` `plan_relay`).

use crate::io::Io;
use crate::password::{authorize, basic, fault_of};
use porter_capability::Claim;
use porter_core::{Credential, EndpointUrl, LoginName, ServiceEndpoint};
use porter_discover::parse_jmap_session;
use porter_http::http::Method;
use porter_http::{Http, request_to};
use porter_sheet::SignInFault;

/// The JMAP endpoint and what the session offers, logged in as `login`.
pub(super) async fn session(
    io: &Io,
    url: &EndpointUrl,
    login: &LoginName,
    credential: &Credential,
) -> Result<(Vec<ServiceEndpoint>, Vec<Claim>), SignInFault> {
    let mut request = request_to(Method::GET, url)
        .and_then(|builder| Ok(builder.body(Vec::new())?))
        .map_err(fault_of)?;
    let authorization = match credential {
        Credential::Password(password) => basic(&login.0, password),
        Credential::Bearer(token) => format!("Bearer {}", token.expose()),
        _ => return Err(SignInFault::Unreadable),
    };
    authorize(&mut request, &authorization).map_err(fault_of)?;
    let response = io.http.send(request).await.map_err(fault_of)?;
    match response.status().as_u16() {
        401 | 403 => return Err(SignInFault::Refused),
        200..=299 => {}
        500..=599 => return Err(SignInFault::Unreachable),
        _ => return Err(SignInFault::Unreadable),
    }
    let body = std::str::from_utf8(response.body()).map_err(|_| SignInFault::Unreadable)?;
    let found = parse_jmap_session(body).map_err(|_| SignInFault::Unreadable)?;
    // The session names the login it knows the account by; the one the person typed is what
    // authenticated, so that is what the relay presents.
    let endpoints = found
        .endpoints
        .into_iter()
        .map(|e| ServiceEndpoint {
            login: login.clone(),
            ..e
        })
        .collect();
    Ok((endpoints, found.claims))
}
