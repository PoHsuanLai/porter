//! A JMAP server a person typed by hand: the session resource at the URL they gave, fetched with
//! the account's credential (HTTP Basic for a password, a bearer for an API token). A refusal is
//! the credential's; what is not a session is unreadable. The relay presents the stored secret
//! the same way.

use crate::{Error, bodiless};
use http::{HeaderValue, Method, Request, Response};
use porter_core::{EndpointUrl, LoginName, ServiceEndpoint};

/// The session request, with the account's `authorization`.
pub fn request(url: &EndpointUrl, authorization: HeaderValue) -> Result<Request<Vec<u8>>, Error> {
    bodiless(Method::GET, url, Some(authorization))
}

/// The session document of the `response`, when the server answered with one.
pub fn session_text(response: &Response<Vec<u8>>) -> Result<&str, Error> {
    match response.status().as_u16() {
        401 | 403 => return Err(Error::Refused),
        200..=299 => {}
        500..=599 => return Err(Error::Unreachable),
        _ => return Err(Error::Unreadable),
    }
    std::str::from_utf8(response.body()).map_err(|_| Error::Unreadable)
}

/// The session's endpoints, logged in as `login`: the session names the login it knows the
/// account by, but the one the person typed is what authenticated, so that is what the relay
/// presents.
pub fn as_login(endpoints: Vec<ServiceEndpoint>, login: &LoginName) -> Vec<ServiceEndpoint> {
    endpoints
        .into_iter()
        .map(|e| ServiceEndpoint {
            login: login.clone(),
            ..e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{Family, Tls};

    fn reply(status: u16, body: &[u8]) -> Response<Vec<u8>> {
        Response::builder()
            .status(status)
            .body(body.to_vec())
            .unwrap()
    }

    #[test]
    fn the_session_is_fetched_with_the_credential() {
        let url = EndpointUrl::parse("https://jmap.example.test/.well-known/jmap").unwrap();
        let request = request(&url, HeaderValue::from_static("Bearer t")).unwrap();
        assert_eq!(request.method(), "GET");
        assert!(request.headers().contains_key(http::header::AUTHORIZATION));
        let plain = EndpointUrl::parse("http://jmap.example.test/").unwrap();
        assert_eq!(
            super::request(&plain, HeaderValue::from_static("x")).unwrap_err(),
            Error::Insecure
        );
    }

    #[test]
    fn a_reply_is_a_session_text_a_refusal_or_a_failure() {
        assert_eq!(session_text(&reply(200, b"{}")), Ok("{}"));
        assert_eq!(session_text(&reply(401, b"")), Err(Error::Refused));
        assert_eq!(session_text(&reply(403, b"")), Err(Error::Refused));
        assert_eq!(session_text(&reply(502, b"")), Err(Error::Unreachable));
        assert_eq!(session_text(&reply(404, b"")), Err(Error::Unreadable));
        assert_eq!(
            session_text(&reply(200, &[0xff, 0xfe])),
            Err(Error::Unreadable)
        );
    }

    #[test]
    fn the_typed_login_replaces_the_one_the_session_names() {
        let endpoint = ServiceEndpoint {
            family: Family::Jmap,
            url: EndpointUrl::parse("https://jmap.example.test/").unwrap(),
            tls: Tls::Implicit,
            login: LoginName("session-name".into()),
        };
        let got = as_login(vec![endpoint], &LoginName("ada".into()));
        assert_eq!(got[0].login.0, "ada");
    }
}
