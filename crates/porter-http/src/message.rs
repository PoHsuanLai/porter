//! Building a request from porter's URLs, and reading headers. The request and response are the
//! `http` crate's (`Request<Vec<u8>>`, `Response<Vec<u8>>`); the URL a request goes to is a porter
//! `WebUrl`, and [`request`] is the one place it becomes the request's `Uri`.

use crate::error::HttpError;
use http::header::{HeaderMap, HeaderName, HeaderValue};
use http::request::Builder;
use http::{Method, Request, Response};
use porter_core::{EndpointUrl, UrlScheme, WebUrl};

/// A request to `url`: the builder with its method and URI set, to which the caller adds headers
/// and a body. The URI is the URL's text; a URL the `http` crate does not take as a URI fails
/// when the body is set.
pub fn request(method: Method, url: &WebUrl) -> Builder {
    Request::builder().method(method).uri(url.as_str())
}

/// A request to an endpoint's URL. A scheme that is not HTTP is `Malformed`, and plain `http`
/// to another computer is `Tls`: no credential is sent in the clear.
pub fn request_to(method: Method, url: &EndpointUrl) -> Result<Builder, HttpError> {
    web_url(url).map(|web| request(method, &web))
}

/// An endpoint's URL as a web URL. A scheme that is not HTTP is `Malformed`, and plain `http`
/// to another computer is `Tls`: no credential is sent in the clear.
pub fn web_url(url: &EndpointUrl) -> Result<WebUrl, HttpError> {
    WebUrl::try_from(url).map_err(|_| match url.origin().scheme {
        UrlScheme::Http => HttpError::Tls,
        _ => HttpError::Malformed,
    })
}

/// A header whose value is a credential: marked sensitive, so the request's `Debug` shows
/// `Sensitive` in its place.
pub trait SensitiveHeader {
    /// The same builder with the header `name: value`, its value marked sensitive. A value that
    /// cannot be a header's fails when the body is set, as any other header does.
    #[must_use]
    fn sensitive_header(self, name: HeaderName, value: &str) -> Self;
}

impl SensitiveHeader for Builder {
    fn sensitive_header(self, name: HeaderName, value: &str) -> Self {
        match HeaderValue::from_str(value) {
            Ok(mut value) => {
                value.set_sensitive(true);
                self.header(name, value)
            }
            Err(_) => self.header(name, value),
        }
    }
}

/// A header read as text, on a request, a response or a header map.
pub trait HeaderText {
    /// The first header named `name` (compared without case), when its value is visible ASCII.
    fn header(&self, name: &str) -> Option<&str>;

    /// The wait a `Retry-After` header asks for, in seconds: its whole seconds, or the time from
    /// now to its HTTP date (none past: `0`). `None` when there is no such header or it is
    /// neither. The one parser the storage replicas and syncd's Graph source share.
    fn retry_after_seconds(&self) -> Option<u32> {
        retry_wait(self.header("retry-after")?, std::time::SystemTime::now())
    }
}

/// The wait a `Retry-After` value asks for at `now`: whole seconds, or an HTTP date (RFC 9110
/// section 5.6.7, read by `httpdate`) less `now`, `0` when the date has passed.
fn retry_wait(value: &str, now: std::time::SystemTime) -> Option<u32> {
    let value = value.trim();
    if let Ok(seconds) = value.parse() {
        return Some(seconds);
    }
    let at = httpdate::parse_http_date(value).ok()?;
    let wait = at.duration_since(now).map_or(0, |wait| wait.as_secs());
    u32::try_from(wait).ok()
}

impl HeaderText for HeaderMap {
    fn header(&self, name: &str) -> Option<&str> {
        self.get(name)?.to_str().ok()
    }
}

impl<B> HeaderText for Request<B> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers().header(name)
    }
}

impl<B> HeaderText for Response<B> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers().header(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;
    use http::header::AUTHORIZATION;

    fn answer(status: u16, headers: &[(&str, &str)]) -> Response<Vec<u8>> {
        headers
            .iter()
            .fold(Response::builder().status(status), |b, (name, value)| {
                b.header(*name, *value)
            })
            .body(Vec::new())
            .expect("response")
    }

    #[test]
    fn retry_after_is_whole_seconds_a_date_or_nothing() {
        let cases = [
            (Some("120"), Some(120)),
            (Some(" 7 "), Some(7)),
            (Some("0"), Some(0)),
            (Some("Wed, 21 Oct 2026 07:28:00 GMT"), Some(0)),
            (Some("not a date"), None),
            (Some("-3"), None),
            (Some("4294967296"), None),
            (Some(""), None),
            (None, None),
        ];
        for (header, want) in cases {
            let headers: Vec<(&str, &str)> =
                header.map(|v| ("Retry-After", v)).into_iter().collect();
            assert_eq!(
                answer(429, &headers).retry_after_seconds(),
                want,
                "{header:?}"
            );
        }
    }

    #[test]
    fn a_request_is_built_up_and_a_response_is_read_without_case() {
        let url = WebUrl::parse("https://cloud.example.org/remote.php/dav/?a=b").expect("url");
        let propfind = Method::from_bytes(b"PROPFIND").expect("method");
        let sent = request(propfind, &url)
            .header("Depth", "1")
            .body(b"<propfind/>".to_vec())
            .expect("request");
        assert_eq!(sent.method().as_str(), "PROPFIND");
        assert_eq!(sent.uri().to_string(), url.as_str());
        assert_eq!(sent.header("depth"), Some("1"));
        assert_eq!(sent.body(), b"<propfind/>");
        let got = answer(207, &[("Content-Type", "application/xml")]);
        assert_eq!(got.header("content-type"), Some("application/xml"));
        assert_eq!(got.header("etag"), None);
        assert!(got.status().is_success());
        assert!(!StatusCode::UNAUTHORIZED.is_success());
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
            let got = request_to(Method::GET, &url).map(|_| ());
            assert_eq!(got, want, "{text}");
        }
        let url = EndpointUrl::parse("https://cloud.example.org/dav/").expect("url");
        let sent = request_to(Method::GET, &url)
            .expect("builder")
            .body(Vec::<u8>::new())
            .expect("request");
        assert_eq!(sent.uri().to_string(), url.as_str());
    }

    #[test]
    fn a_credential_never_shows_in_debug_and_a_bad_one_fails_the_build() {
        let url = WebUrl::parse("https://cloud.example.org/").expect("url");
        let sent = request(Method::GET, &url)
            .sensitive_header(AUTHORIZATION, "Bearer abc123")
            .header("Content-Type", "text/xml")
            .body(Vec::<u8>::new())
            .expect("request");
        let text = format!("{sent:?}");
        assert!(!text.contains("abc123"), "{text}");
        assert!(text.contains("text/xml"), "{text}");
        assert_eq!(sent.header("authorization"), Some("Bearer abc123"));
        let bad = request(Method::GET, &url)
            .sensitive_header(AUTHORIZATION, "Bearer abc\r\nX: y")
            .body(Vec::<u8>::new());
        assert!(bad.is_err());
    }

    #[test]
    fn a_retry_after_date_is_the_time_from_now_to_it() {
        use std::time::{Duration, UNIX_EPOCH};
        // Sun, 06 Nov 1994 08:49:37 GMT is 784111777 seconds after the epoch.
        let now = UNIX_EPOCH + Duration::from_secs(784_111_777 - 90);
        let cases = [
            ("Sun, 06 Nov 1994 08:49:37 GMT", Some(90)),
            ("Sunday, 06-Nov-94 08:49:37 GMT", Some(90)),
            ("Sun Nov  6 08:49:37 1994", Some(90)),
            ("Sun, 06 Nov 1994 08:48:07 GMT", Some(0)),
            ("Sun, 06 Nov 1994 08:40:00 GMT", Some(0)),
            ("Fri, 31 Dec 9999 23:59:59 GMT", None),
            ("120", Some(120)),
            ("soon", None),
        ];
        for (value, want) in cases {
            assert_eq!(retry_wait(value, now), want, "{value}");
        }
    }
}
