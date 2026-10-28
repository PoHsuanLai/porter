//! What the JSON backends share: a request with a JSON body, the exchange that turns a status
//! that is not a success into a [`CalendarError`], the reading of a `DELETE`'s answer, a body read
//! as JSON, and the URL pieces a store's ids and query values are written into.

use crate::error::CalendarError;
use porter_http::http::header::{HeaderName, HeaderValue, IF_MATCH};
use porter_http::http::{Method, Request, Response};
use porter_http::{Http, HttpError, request as builder};
use porter_ids::WebUrl;
use serde_json::Value;

/// A request to `url` with `body` as JSON.
pub(crate) fn request(
    method: Method,
    url: &WebUrl,
    body: &Value,
) -> Result<Request<Vec<u8>>, CalendarError> {
    Ok(builder(method, url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(body.to_string().into_bytes())
        .map_err(HttpError::from)?)
}

/// A request to `url` with no body.
pub(crate) fn bare(method: Method, url: &WebUrl) -> Result<Request<Vec<u8>>, CalendarError> {
    Ok(builder(method, url)
        .header("Accept", "application/json")
        .body(Vec::new())
        .map_err(HttpError::from)?)
}

/// `request` with the header `name: value` added; a value a header cannot carry is never sent.
pub(crate) fn with_header(
    mut request: Request<Vec<u8>>,
    name: HeaderName,
    value: &str,
) -> Result<Request<Vec<u8>>, CalendarError> {
    let value = HeaderValue::from_str(value).map_err(|_| HttpError::Malformed)?;
    request.headers_mut().insert(name, value);
    Ok(request)
}

/// `request` made conditional on the stored version still being `etag` (`If-Match`).
pub(crate) fn if_match(
    request: Request<Vec<u8>>,
    etag: &str,
) -> Result<Request<Vec<u8>>, CalendarError> {
    with_header(request, IF_MATCH, etag)
}

/// Send `request`; a response whose status is not a success is the error it stands for.
pub(crate) async fn exchange<H: Http>(
    http: &H,
    request: Request<Vec<u8>>,
    what: &'static str,
) -> Result<Response<Vec<u8>>, CalendarError> {
    let response = http.send(request).await?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(CalendarError::of_status(response.status().as_u16(), what))
    }
}

/// Whether a `DELETE` removed something: `true` for a success, `false` for an event that was not
/// there, the error a status stands for otherwise.
pub(crate) fn removed(
    response: &Response<Vec<u8>>,
    what: &'static str,
) -> Result<bool, CalendarError> {
    match response.status().as_u16() {
        200..=299 => Ok(true),
        404 | 410 => Ok(false),
        status => Err(CalendarError::of_status(status, what)),
    }
}

/// The response's body as JSON.
pub(crate) fn body(response: &Response<Vec<u8>>) -> Result<Value, CalendarError> {
    serde_json::from_slice(response.body()).map_err(|e| CalendarError::Malformed(e.to_string()))
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
