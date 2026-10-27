//! What the JSON backends share: a request with a JSON body, the exchange that turns a status
//! that is not a success into a [`CalendarError`], the reading of a `DELETE`'s answer, a body read
//! as JSON, and the URL pieces a store's ids and query values are written into.

use crate::error::CalendarError;
use porter_core::WebUrl;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use serde_json::Value;

/// A request to `url` with `body` as JSON.
pub(crate) fn request(method: Method, url: WebUrl, body: &Value) -> HttpRequest {
    HttpRequest::new(method, url)
        .with_header("Content-Type", "application/json")
        .with_header("Accept", "application/json")
        .with_body(body.to_string())
}

/// A request to `url` with no body.
pub(crate) fn bare(method: Method, url: WebUrl) -> HttpRequest {
    HttpRequest::new(method, url).with_header("Accept", "application/json")
}

/// Send `request`; a response whose status is not a success is the error it stands for.
pub(crate) async fn exchange<H: Http>(
    http: &H,
    request: HttpRequest,
    what: &'static str,
) -> Result<HttpResponse, CalendarError> {
    let response = http.send(request).await?;
    if response.status.is_success() {
        Ok(response)
    } else {
        Err(CalendarError::of_status(response.status.0, what))
    }
}

/// Whether a `DELETE` removed something: `true` for a success, `false` for an event that was not
/// there, the error a status stands for otherwise.
pub(crate) fn removed(response: &HttpResponse, what: &'static str) -> Result<bool, CalendarError> {
    match response.status.0 {
        200..=299 => Ok(true),
        404 | 410 => Ok(false),
        status => Err(CalendarError::of_status(status, what)),
    }
}

/// The response's body as JSON.
pub(crate) fn body(response: &HttpResponse) -> Result<Value, CalendarError> {
    serde_json::from_slice(&response.body).map_err(|e| CalendarError::Malformed(e.to_string()))
}

/// The URL written `text`, or why it is not one.
pub(crate) fn url(text: &str) -> Result<WebUrl, CalendarError> {
    WebUrl::parse(text).map_err(|e| CalendarError::Malformed(e.to_string()))
}

/// `text` percent-encoded for a path segment or a query value: everything but RFC 3986's
/// unreserved characters.
pub(crate) fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A string field of `value`, when it has one.
pub(crate) fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unreserved_characters_go_unencoded() {
        assert_eq!(encode("a-Z_0.~"), "a-Z_0.~");
        assert_eq!(encode("uid@host/x y'"), "uid%40host%2Fx%20y%27");
        assert_eq!(encode("é"), "%C3%A9");
    }
}
