//! A JMAP server a person typed by hand, sent: the session resource at the URL they gave,
//! fetched with HTTP Basic (the login name and the password) or, for an API token, a bearer,
//! read the way discovery reads a session it found. The request and the reading of the answer
//! are generic-proto's.

use super::fault;
use crate::io::Io;
use crate::password::{basic, fault_of, sensitive};
use generic_proto::jmap;
use porter_capability::Claim;
use porter_core::{Credential, EndpointUrl, LoginName, ServiceEndpoint};
use porter_discover::parse_jmap_session;
use porter_http::Http;
use porter_sheet::SignInFault;

/// The JMAP endpoint and what the session offers, logged in as `login`.
pub(super) async fn session(
    io: &Io,
    url: &EndpointUrl,
    login: &LoginName,
    credential: &Credential,
) -> Result<(Vec<ServiceEndpoint>, Vec<Claim>), SignInFault> {
    let authorization = match credential {
        Credential::Password(password) => basic(&login.0, password),
        Credential::Bearer(token) => format!("Bearer {}", token.expose()),
        _ => return Err(SignInFault::Unreadable),
    };
    let request =
        jmap::request(url, sensitive(&authorization).map_err(fault_of)?).map_err(fault)?;
    let response = io.http.send(request).await.map_err(fault_of)?;
    let body = jmap::session_text(&response).map_err(fault)?;
    let found = parse_jmap_session(body).map_err(|_| SignInFault::Unreadable)?;
    Ok((jmap::as_login(found.endpoints, login), found.claims))
}
