//! The least HTTP/1.1 a model client needs, over any byte stream: read one request (a
//! `Content-Length` or chunked body, `Expect: 100-continue`), write one response (a JSON body, or
//! an event stream sent chunked) and close. Every response says `Connection: close`, so there is
//! no pipelining and no state between requests.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest request body read: a long agent conversation with its tool results.
pub const MAX_BODY: usize = 64 << 20;
/// The largest request head.
const MAX_HEAD: usize = 64 << 10;

/// One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET`, `POST`, ...
    pub method: String,
    /// The target as sent: path and query.
    pub target: String,
    headers: Vec<(String, String)>,
    /// The body, decoded.
    pub body: Vec<u8>,
}

/// Why a request could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// The peer closed before a request arrived.
    Closed,
    /// Not an HTTP/1.1 request.
    Malformed,
    /// The head or body is over the limit.
    TooLarge,
    /// The reader of the head would not have this request: its body was not read.
    Refused,
}

impl Request {
    /// The target without its query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    /// The value of the header `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(have, _)| have.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The secret the client presented: `x-api-key`, else a bearer token.
    pub fn presented(&self) -> Option<&str> {
        self.header("x-api-key").or_else(|| {
            self.header("authorization").and_then(|value| {
                let (scheme, rest) = value.split_once(' ')?;
                scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
            })
        })
    }

    /// A request for tests and for the dispatcher's own use.
    pub fn new(method: &str, target: &str, headers: &[(&str, &str)], body: &[u8]) -> Self {
        Self {
            method: method.to_owned(),
            target: target.to_owned(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: body.to_vec(),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_head(head: &str) -> Result<(String, String, Vec<(String, String)>), ReadError> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next().ok_or(ReadError::Malformed)?.split(' ');
    let (method, target, version) = (first.next(), first.next(), first.next());
    let (Some(method), Some(target), Some(version)) = (method, target, version) else {
        return Err(ReadError::Malformed);
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(ReadError::Malformed);
    }
    let headers = lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
                .ok_or(ReadError::Malformed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((method.to_owned(), target.to_owned(), headers))
}

/// Reads at least `want` bytes of `buffer`, from `stream`.
async fn fill<S: AsyncRead + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
    want: usize,
) -> Result<(), ReadError> {
    let mut chunk = [0_u8; 16 * 1024];
    while buffer.len() < want {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|_| ReadError::Closed)?;
        if n == 0 {
            return Err(if buffer.is_empty() {
                ReadError::Closed
            } else {
                ReadError::Malformed
            });
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() > MAX_BODY + MAX_HEAD {
            return Err(ReadError::TooLarge);
        }
    }
    Ok(())
}

/// Reads one chunked body that starts at `buffer[0]`.
async fn read_chunked<S: AsyncRead + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    loop {
        let line_end = loop {
            if let Some(at) = find(buffer, b"\r\n") {
                break at;
            }
            let want = buffer.len() + 1;
            fill(stream, buffer, want).await?;
        };
        let size_text = std::str::from_utf8(&buffer[..line_end])
            .map_err(|_| ReadError::Malformed)?
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let size = usize::from_str_radix(&size_text, 16).map_err(|_| ReadError::Malformed)?;
        buffer.drain(..line_end + 2);
        if size == 0 {
            // Trailers, if any, end at a blank line; this client sends none that matter.
            return Ok(body);
        }
        if body.len() + size > MAX_BODY {
            return Err(ReadError::TooLarge);
        }
        fill(stream, buffer, size + 2).await?;
        body.extend_from_slice(&buffer[..size]);
        buffer.drain(..size + 2);
    }
}

/// Reads one request. `admit` sees the request line and headers (the body not yet read) and
/// says whether to go on: a refused request's body is never read, so a client without the
/// session's token cannot make the daemon take in a large body.
pub async fn read_request<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    admit: impl FnOnce(&Request) -> bool,
) -> Result<Request, ReadError> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(at) = find(&buffer, b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > MAX_HEAD {
            return Err(ReadError::TooLarge);
        }
        let want = buffer.len() + 1;
        fill(stream, &mut buffer, want).await?;
    };
    let head = std::str::from_utf8(&buffer[..head_end]).map_err(|_| ReadError::Malformed)?;
    let (method, target, headers) = parse_head(head)?;
    buffer.drain(..head_end + 4);
    let head_only = Request {
        method: method.clone(),
        target: target.clone(),
        headers: headers.clone(),
        body: Vec::new(),
    };
    if !admit(&head_only) {
        return Err(ReadError::Refused);
    }
    let named = |name: &str| {
        headers
            .iter()
            .find(|(have, _)| have.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    if named("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await;
    }
    let body = if named("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        read_chunked(stream, &mut buffer).await?
    } else {
        let length = match named("content-length") {
            None => 0,
            Some(text) => text.parse::<usize>().map_err(|_| ReadError::Malformed)?,
        };
        if length > MAX_BODY {
            return Err(ReadError::TooLarge);
        }
        fill(stream, &mut buffer, length).await?;
        buffer.truncate(length);
        buffer
    };
    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        529 => "Overloaded",
        _ => "Error",
    }
}

fn head(status: u16, content_type: &str, extra: &[(&str, String)], framing: &str) -> String {
    let mut text = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\n{framing}\r\nConnection: close\r\n",
        reason(status)
    );
    for (name, value) in extra {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    text
}

/// Writes a whole response with a JSON body.
pub async fn write_json<S: AsyncWrite + Unpin>(
    stream: &mut S,
    status: u16,
    extra: &[(&str, String)],
    body: &str,
) -> std::io::Result<()> {
    let framing = format!("Content-Length: {}", body.len());
    let mut out = head(status, "application/json", extra, &framing).into_bytes();
    out.extend_from_slice(body.as_bytes());
    stream.write_all(&out).await?;
    stream.flush().await
}

/// Starts an event stream: the head, chunked, so [`write_chunk`] may follow.
pub async fn start_stream<S: AsyncWrite + Unpin>(stream: &mut S) -> std::io::Result<()> {
    let text = head(
        200,
        "text/event-stream",
        &[("Cache-Control", "no-cache".to_owned())],
        "Transfer-Encoding: chunked",
    );
    stream.write_all(text.as_bytes()).await?;
    stream.flush().await
}

/// One chunk of a chunked body, sent at once.
pub async fn write_chunk<S: AsyncWrite + Unpin>(
    stream: &mut S,
    data: &[u8],
) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let mut out = format!("{:x}\r\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    stream.write_all(&out).await?;
    stream.flush().await
}

/// Ends a chunked body.
pub async fn end_stream<S: AsyncWrite + Unpin>(stream: &mut S) -> std::io::Result<()> {
    stream.write_all(b"0\r\n\r\n").await?;
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read(bytes: &[u8]) -> Result<Request, ReadError> {
        let (mut ours, mut theirs) = tokio::io::duplex(1 << 20);
        let sent = bytes.to_vec();
        tokio::spawn(async move {
            let _ = theirs.write_all(&sent).await;
            let _ = theirs.shutdown().await;
        });
        read_request(&mut ours, |_| true).await
    }

    #[tokio::test]
    async fn a_content_length_body_is_read_whole() {
        let request = read(
            b"POST /v1/messages?beta=true HTTP/1.1\r\nHost: x\r\nX-Api-Key: tok\r\nContent-Length: 7\r\n\r\n{\"a\":1}",
        )
        .await
        .expect("request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path(), "/v1/messages");
        assert_eq!(request.header("x-API-key"), Some("tok"));
        assert_eq!(request.presented(), Some("tok"));
        assert_eq!(request.body, b"{\"a\":1}");
    }

    #[tokio::test]
    async fn a_chunked_body_is_joined_and_a_bearer_is_presented() {
        let request = read(
            b"POST /v1/chat/completions HTTP/1.1\r\nAuthorization: Bearer abc\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n4;ext=1\r\n\":1}\r\n0\r\n\r\n",
        )
        .await
        .expect("request");
        assert_eq!(request.presented(), Some("abc"));
        assert_eq!(request.body, b"{\"a\":1}");
    }

    #[tokio::test]
    async fn a_refused_head_leaves_its_body_unread() {
        let (mut ours, mut theirs) = tokio::io::duplex(1 << 10);
        tokio::spawn(async move {
            // A body far larger than the pipe: it would block if anyone tried to read it all.
            let _ = theirs
                .write_all(b"POST / HTTP/1.1\r\nContent-Length: 50000000\r\n\r\nxxxx")
                .await;
            std::future::pending::<()>().await;
        });
        let refused = read_request(&mut ours, |head| head.header("x-api-key").is_some()).await;
        assert_eq!(refused.err(), Some(ReadError::Refused));
    }

    #[tokio::test]
    async fn the_malformed_and_the_oversized_are_refused() {
        let cases: [(&[u8], ReadError); 5] = [
            (b"", ReadError::Closed),
            (b"GARBAGE\r\n\r\n", ReadError::Malformed),
            (b"GET nowhere HTTP/1.1\r\n\r\n", ReadError::Malformed),
            (
                b"POST / HTTP/1.1\r\nContent-Length: 99999999999\r\n\r\n",
                ReadError::TooLarge,
            ),
            (
                b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort",
                ReadError::Malformed,
            ),
        ];
        for (bytes, want) in cases {
            assert_eq!(read(bytes).await.err(), Some(want), "{bytes:?}");
        }
    }

    #[tokio::test]
    async fn a_json_response_and_a_stream_are_framed() {
        let mut out = Vec::new();
        write_json(&mut out, 429, &[("retry-after", "3".into())], "{}")
            .await
            .expect("write");
        let text = String::from_utf8(out).expect("text");
        assert!(
            text.starts_with("HTTP/1.1 429 Too Many Requests\r\n"),
            "{text}"
        );
        assert!(text.contains("Content-Length: 2\r\n") && text.contains("retry-after: 3\r\n"));
        assert!(text.ends_with("\r\n\r\n{}"));
        let mut out = Vec::new();
        start_stream(&mut out).await.expect("head");
        write_chunk(&mut out, b"data: x\n\n").await.expect("chunk");
        end_stream(&mut out).await.expect("end");
        let text = String::from_utf8(out).expect("text");
        assert!(text.contains("Transfer-Encoding: chunked"), "{text}");
        assert!(text.ends_with("9\r\ndata: x\n\n\r\n0\r\n\r\n"), "{text}");
    }
}
