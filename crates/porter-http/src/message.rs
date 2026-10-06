//! A request and its response as values.

use crate::error::HttpError;
use crate::headers::{Header, HeaderName};
use porter_core::{EndpointUrl, UrlScheme, WebUrl};

/// The methods porter sends: HTTP's, and WebDAV's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// `GET`.
    Get,
    /// `HEAD`.
    Head,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// `OPTIONS`.
    Options,
    /// WebDAV `PROPFIND`.
    Propfind,
    /// WebDAV `REPORT` (CalDAV, CardDAV, sync-collection).
    Report,
    /// WebDAV `MKCOL`.
    Mkcol,
}

impl Method {
    /// The method's token on the wire.
    pub fn token(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Options => "OPTIONS",
            Method::Propfind => "PROPFIND",
            Method::Report => "REPORT",
            Method::Mkcol => "MKCOL",
        }
    }
}

/// An endpoint's URL as a web URL. A scheme that is not HTTP is `Malformed`, and plain `http`
/// to another computer is `Tls`: no credential is sent in the clear.
pub fn web_url(url: &EndpointUrl) -> Result<WebUrl, HttpError> {
    WebUrl::try_from(url).map_err(|_| match url.origin().scheme {
        UrlScheme::Http => HttpError::Tls,
        _ => HttpError::Malformed,
    })
}

/// One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// The method.
    pub method: Method,
    /// Where to: `https`, or `http` to this computer, with a path and a query.
    pub url: WebUrl,
    /// The headers, in order.
    pub headers: Vec<Header>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// A request with no headers and no body.
    pub fn new(method: Method, url: WebUrl) -> Self {
        Self {
            method,
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// A request to an endpoint's URL. A scheme that is not HTTP is `Malformed`, and plain
    /// `http` to another computer is `Tls`: no credential is sent in the clear.
    pub fn to(method: Method, url: &EndpointUrl) -> Result<Self, HttpError> {
        web_url(url).map(|web| Self::new(method, web))
    }

    /// The same request with one more header.
    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push(Header::new(name, value));
        self
    }

    /// The same request with this body.
    pub fn with_body(self, body: impl Into<Vec<u8>>) -> Self {
        Self {
            body: body.into(),
            ..self
        }
    }
}

/// An HTTP status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Status(pub u16);

impl Status {
    /// Whether the status is 2xx.
    pub fn is_success(self) -> bool {
        (200..300).contains(&self.0)
    }
}

/// One response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// The status.
    pub status: Status,
    /// The headers, in order.
    pub headers: Vec<Header>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// The first header named `name` (compared without case).
    pub fn header(&self, name: &str) -> Option<&str> {
        let wanted = HeaderName::new(name);
        self.headers
            .iter()
            .find(|h| h.name == wanted)
            .map(|h| h.value.0.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_built_up_and_a_response_is_read_without_case() {
        let url = WebUrl::parse("https://cloud.example.org/remote.php/dav/?a=b").expect("url");
        let request = HttpRequest::new(Method::Propfind, url)
            .with_header("Depth", "1")
            .with_body("<propfind/>");
        assert_eq!(request.method.token(), "PROPFIND");
        assert_eq!(request.body, b"<propfind/>");
        let response = HttpResponse {
            status: Status(207),
            headers: vec![Header::new("Content-Type", "application/xml")],
            body: vec![],
        };
        assert_eq!(response.header("content-type"), Some("application/xml"));
        assert_eq!(response.header("etag"), None);
        assert!(response.status.is_success());
        assert!(!Status(401).is_success());
    }

    #[test]
    fn a_request_to_an_endpoint_refuses_what_is_not_web() {
        let cases = [
            ("https://cloud.example.org/dav/", Ok(())),
            ("http://127.0.0.1:8080/dav/", Ok(())),
            ("http://cloud.example.org/dav/", Err(HttpError::Tls)),
            ("imaps://mail.example.org", Err(HttpError::Malformed)),
        ];
        for (text, want) in cases {
            let url = EndpointUrl::parse(text).expect(text);
            let got = HttpRequest::to(Method::Get, &url).map(|r| r.url.to_string());
            assert_eq!(got.map(|_| ()), want, "{text}");
        }
        let url = EndpointUrl::parse("https://cloud.example.org/dav/").expect("url");
        let request = HttpRequest::to(Method::Get, &url).expect("request");
        assert_eq!(request.url.as_str(), url.as_str());
    }
}
