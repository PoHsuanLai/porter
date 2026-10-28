//! Taking an app password back: `DELETE /ocs/v2.php/core/apppassword`, authenticated with the
//! app password itself, deletes the one it authenticates with.

use crate::{Error, authorized, web};
use http::header::ACCEPT;
use http::{HeaderValue, Method, Request, Response};
use porter_core::EndpointUrl;

/// What the server said to the revoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revoked {
    /// The app password is gone, or the server already refuses it.
    Done,
    /// The server has no such endpoint (before Nextcloud 13): the person takes it back here.
    Manual(EndpointUrl),
}

/// The revoke request, with the app password's `authorization`.
pub fn request(
    server: &EndpointUrl,
    authorization: HeaderValue,
) -> Result<Request<Vec<u8>>, Error> {
    let url = web(&crate::paths::app_password(server).ok_or(Error::Unreadable)?)?;
    let request = Request::builder()
        .method(Method::DELETE)
        .uri(url.as_str())
        .header("OCS-APIRequest", "true")
        .header(ACCEPT, "application/json")
        .body(Vec::new())
        .map_err(|_| Error::Unreadable)?;
    Ok(authorized(request, authorization))
}

/// What the `response` to the revoke came to. A server that already refuses the password has
/// nothing left to revoke.
pub fn revoked(server: &EndpointUrl, response: &Response<Vec<u8>>) -> Result<Revoked, Error> {
    match response.status().as_u16() {
        200..=299 | 401 | 403 => Ok(Revoked::Done),
        404 => crate::paths::security_settings(server)
            .map(Revoked::Manual)
            .ok_or(Error::Unreadable),
        500..=599 => Err(Error::Unreachable),
        _ => Err(Error::Unreadable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16) -> Response<Vec<u8>> {
        Response::builder().status(status).body(Vec::new()).unwrap()
    }

    #[test]
    fn the_revoke_is_a_delete_with_the_credential() {
        let server = EndpointUrl::parse("https://cloud.example.org").unwrap();
        let mut auth = HeaderValue::from_static("Basic abc");
        auth.set_sensitive(true);
        let request = request(&server, auth).unwrap();
        assert_eq!(request.method(), "DELETE");
        assert_eq!(
            request.uri(),
            "https://cloud.example.org/ocs/v2.php/core/apppassword"
        );
        assert!(request.headers()[http::header::AUTHORIZATION].is_sensitive());
        assert!(!format!("{request:?}").contains("abc"));
    }

    #[test]
    fn a_revoke_is_done_manual_or_failed() {
        let server = EndpointUrl::parse("https://cloud.example.org").unwrap();
        for status in [200, 204, 401, 403] {
            assert_eq!(
                revoked(&server, &reply(status)),
                Ok(Revoked::Done),
                "{status}"
            );
        }
        assert_eq!(
            revoked(&server, &reply(404)),
            Ok(Revoked::Manual(
                EndpointUrl::parse("https://cloud.example.org/settings/user/security").unwrap()
            ))
        );
        assert_eq!(revoked(&server, &reply(500)), Err(Error::Unreachable));
        assert_eq!(revoked(&server, &reply(400)), Err(Error::Unreadable));
    }
}
