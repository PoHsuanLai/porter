//! A request and its response as values.

use crate::headers::{Header, HeaderName};
use porter_core::EndpointUrl;

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

/// One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// The method.
    pub method: Method,
    /// Where to. An endpoint URL, so a request goes to a scheme and host porter knows.
    pub url: EndpointUrl,
    /// The headers, in order.
    pub headers: Vec<Header>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// A request with no headers and no body.
    pub fn new(method: Method, url: EndpointUrl) -> Self {
        Self {
            method,
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }
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
        let url = EndpointUrl::parse("https://cloud.example.org/remote.php/dav/").expect("url");
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
}
