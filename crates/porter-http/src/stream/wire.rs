//! HTTP/1.1 on a byte stream: a request written as bytes and a response read back, framed by
//! `Content-Length`, chunks or the end of the stream. Only what a WebDAV exchange needs: no
//! redirects, no upgrade, no compression (the requests say `Accept-Encoding: identity`).

use crate::{Header, HttpError, HttpRequest, HttpResponse, Method, Status};
use porter_core::stream::ByteStream;

/// The most a response head may take.
const MAX_HEAD: usize = 64 * 1024;

/// What was read, and whether the stream can carry another exchange.
#[derive(Debug)]
pub struct Exchange {
    /// The response.
    pub response: HttpResponse,
    /// Whether the server kept the stream open and the body was framed.
    pub reusable: bool,
}

/// A request as bytes: origin-form target, `Host` from `authority`, our own framing headers.
pub fn encode(request: &HttpRequest, authority: &str) -> Vec<u8> {
    let url = request.url.path();
    let target = match request.url.query() {
        Some(query) => format!("{url}?{query}"),
        None => url.to_owned(),
    };
    let mut head = format!(
        "{} {target} HTTP/1.1\r\nHost: {authority}\r\nAccept-Encoding: identity\r\nConnection: keep-alive\r\n",
        request.method.token()
    );
    for header in request.headers.iter().filter(|h| !ours(h)) {
        head.push_str(&format!("{}: {}\r\n", header.name.as_str(), header.value.0));
    }
    let bodied = !request.body.is_empty()
        || matches!(
            request.method,
            Method::Put | Method::Post | Method::Propfind | Method::Report
        );
    if bodied {
        head.push_str(&format!("Content-Length: {}\r\n", request.body.len()));
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&request.body);
    bytes
}

/// Headers the encoder writes itself.
fn ours(header: &Header) -> bool {
    matches!(
        header.name.as_str(),
        "host" | "content-length" | "connection" | "transfer-encoding" | "accept-encoding"
    )
}

/// Reads one response for a request made with `method`. `max_body` caps what is kept.
pub async fn read_response<S: ByteStream>(
    stream: &mut S,
    method: Method,
    max_body: usize,
) -> Result<Exchange, HttpError> {
    let mut buf = Vec::new();
    let end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at;
        }
        if buf.len() > MAX_HEAD {
            return Err(HttpError::Malformed);
        }
        fill(stream, &mut buf).await?;
    };
    let head = String::from_utf8(buf[..end].to_vec()).map_err(|_| HttpError::Malformed)?;
    let mut rest = buf.split_off(end + 4);
    let (status, headers) = parse_head(&head)?;
    let header = |name: &str| {
        headers
            .iter()
            .find(|h| h.name.as_str() == name)
            .map(|h| h.value.0.as_str())
    };
    let closes = header("connection").is_some_and(|v| v.eq_ignore_ascii_case("close"));
    let chunked = header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    let length = header("content-length").and_then(|v| v.trim().parse::<usize>().ok());
    let bodiless = method == Method::Head || matches!(status.0, 100..=199 | 204 | 304);
    let (body, framed) = match (bodiless, chunked, length) {
        (true, _, _) => (Vec::new(), true),
        (false, true, _) => (read_chunked(stream, &mut rest, max_body).await?, true),
        (false, false, Some(n)) => {
            if n > max_body {
                return Err(HttpError::TooLarge);
            }
            while rest.len() < n {
                fill(stream, &mut rest).await?;
            }
            let after = rest.split_off(n);
            let body = std::mem::replace(&mut rest, after);
            (body, true)
        }
        (false, false, None) => {
            // No framing: the body runs to the end of the stream.
            loop {
                if rest.len() > max_body {
                    return Err(HttpError::TooLarge);
                }
                match fill(stream, &mut rest).await {
                    Ok(()) => {}
                    Err(HttpError::Unreachable) => break,
                    Err(other) => return Err(other),
                }
            }
            (std::mem::take(&mut rest), false)
        }
    };
    Ok(Exchange {
        response: HttpResponse {
            status,
            headers,
            body,
        },
        reusable: framed && !closes && rest.is_empty(),
    })
}

/// What a failed read or write of the stream means: a stream that gave up waiting
/// ([`std::io::ErrorKind::TimedOut`], which the stream's owner sets as its idle limit) is
/// `TimedOut`, never mistaken for a connection the server closed.
pub(crate) fn io_fault(error: &std::io::Error) -> HttpError {
    match error.kind() {
        std::io::ErrorKind::TimedOut => HttpError::TimedOut,
        _ => HttpError::Unreachable,
    }
}

/// Reads at least one more byte into `buf`; the end of the stream is `Unreachable`, and a read
/// the stream gave up on is `TimedOut`.
async fn fill<S: ByteStream>(stream: &mut S, buf: &mut Vec<u8>) -> Result<(), HttpError> {
    let mut chunk = [0u8; 16 * 1024];
    match stream.read(&mut chunk).await {
        Err(error) => Err(io_fault(&error)),
        Ok(0) => Err(HttpError::Unreachable),
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            Ok(())
        }
    }
}

fn parse_head(head: &str) -> Result<(Status, Vec<Header>), HttpError> {
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .filter(|line| line.starts_with("HTTP/1."))
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(HttpError::Malformed)?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| Header::new(name.trim(), value.trim()))
        .collect();
    Ok((Status(status), headers))
}

/// A chunked body, its trailers dropped. `rest` holds what is already read.
async fn read_chunked<S: ByteStream>(
    stream: &mut S,
    rest: &mut Vec<u8>,
    max_body: usize,
) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    loop {
        let line = take_line(stream, rest).await?;
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| HttpError::Malformed)?;
        if size == 0 {
            // Trailers end at an empty line.
            while !take_line(stream, rest).await?.is_empty() {}
            return Ok(body);
        }
        if body.len().saturating_add(size) > max_body {
            return Err(HttpError::TooLarge);
        }
        while rest.len() < size + 2 {
            fill(stream, rest).await?;
        }
        body.extend_from_slice(&rest[..size]);
        if &rest[size..size + 2] != b"\r\n" {
            return Err(HttpError::Malformed);
        }
        rest.drain(..size + 2);
    }
}

async fn take_line<S: ByteStream>(stream: &mut S, rest: &mut Vec<u8>) -> Result<String, HttpError> {
    loop {
        if let Some(at) = find(rest, b"\r\n") {
            let line = String::from_utf8(rest[..at].to_vec()).map_err(|_| HttpError::Malformed)?;
            rest.drain(..at + 2);
            return Ok(line);
        }
        if rest.len() > MAX_HEAD {
            return Err(HttpError::Malformed);
        }
        fill(stream, rest).await?;
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::WebUrl;
    use porter_core::stream::duplex;

    fn request(method: Method, url: &str) -> HttpRequest {
        HttpRequest::new(method, WebUrl::parse(url).expect("url"))
    }

    #[test]
    fn a_request_is_origin_form_with_its_own_framing() {
        let put = request(Method::Put, "http://127.0.0.1:9/dav/a%20b.txt?x=1")
            .with_header("If-Match", "\"e1\"")
            .with_header("Content-Length", "999")
            .with_header("Host", "evil.test")
            .with_body("hello");
        let text = String::from_utf8(encode(&put, "127.0.0.1:9")).expect("text");
        assert_eq!(
            text,
            "PUT /dav/a%20b.txt?x=1 HTTP/1.1\r\nHost: 127.0.0.1:9\r\nAccept-Encoding: identity\r\n\
             Connection: keep-alive\r\nif-match: \"e1\"\r\nContent-Length: 5\r\n\r\nhello"
        );
        let get = String::from_utf8(encode(&request(Method::Get, "http://127.0.0.1:9/a"), "h"))
            .expect("text");
        assert!(!get.contains("Content-Length"), "{get}");
        let propfind = String::from_utf8(encode(
            &request(Method::Propfind, "http://127.0.0.1:9/a"),
            "h",
        ))
        .expect("text");
        assert!(propfind.contains("Content-Length: 0\r\n"), "{propfind}");
    }

    /// The bytes `server` sends, read as the response to a request made with `method`.
    async fn read(method: Method, server: &[u8], max: usize) -> Result<Exchange, HttpError> {
        let (mut client, mut peer) = duplex(64);
        let bytes = server.to_vec();
        let writer = async move {
            for piece in bytes.chunks(7) {
                peer.write_all(piece).await.expect("write");
            }
            peer
        };
        let (result, _peer) = tokio::join!(read_response(&mut client, method, max), writer);
        result
    }

    #[tokio::test]
    async fn responses_are_framed_by_length_chunks_or_the_end_of_the_stream() {
        const BY_LENGTH: &[u8] = b"HTTP/1.1 207 Multi-Status\r\nContent-Length: 5\r\nContent-Type: application/xml\r\n\r\nhello";
        const CHUNKED: &[u8] = b"HTTP/1.1 207 Multi-Status\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n4;ext=1\r\ndefg\r\n0\r\nTrailer: x\r\n\r\n";
        let by_length = read(Method::Propfind, BY_LENGTH, 100)
            .await
            .expect("length");
        assert_eq!(
            (
                by_length.response.status,
                by_length.response.body.as_slice(),
                by_length.reusable
            ),
            (Status(207), b"hello".as_slice(), true)
        );
        assert_eq!(
            by_length.response.header("content-type"),
            Some("application/xml")
        );
        let chunked = read(Method::Propfind, CHUNKED, 100).await.expect("chunked");
        assert_eq!(
            (chunked.response.body.as_slice(), chunked.reusable),
            (b"abcdefg".as_slice(), true)
        );
    }

    #[tokio::test]
    async fn a_body_with_no_framing_ends_with_the_stream_and_cannot_be_reused() {
        let (mut client, mut peer) = duplex(64);
        peer.write_all(b"HTTP/1.1 200 OK\r\n\r\nrest of it")
            .await
            .expect("write");
        drop(peer);
        let got = read_response(&mut client, Method::Get, 100)
            .await
            .expect("read");
        assert_eq!(
            (got.response.body.as_slice(), got.reusable),
            (b"rest of it".as_slice(), false)
        );
    }

    #[tokio::test]
    async fn no_body_for_head_204_and_close_is_not_reusable() {
        let head = read(
            Method::Head,
            b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n",
            100,
        )
        .await
        .expect("head");
        assert_eq!((head.response.body.len(), head.reusable), (0, true));
        let none = read(Method::Delete, b"HTTP/1.1 204 No Content\r\n\r\n", 100)
            .await
            .expect("204");
        assert_eq!(
            (none.response.status, none.response.body.len()),
            (Status(204), 0)
        );
        let close = read(
            Method::Get,
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
            100,
        )
        .await
        .expect("close");
        assert!(!close.reusable);
    }

    #[tokio::test]
    async fn what_is_not_http_or_too_large_is_refused() {
        const CASES: &[(&[u8], usize, HttpError)] = &[
            (b"SSH-2.0-x\r\n\r\n", 100, HttpError::Malformed),
            (b"HTTP/1.1 abc OK\r\n\r\n", 100, HttpError::Malformed),
            (b"HTTP/1.1 200 OK\r\nContent-Length: 500\r\n\r\n", 100, HttpError::TooLarge),
            (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n", 4, HttpError::TooLarge),
            (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n", 100, HttpError::Malformed),
        ];
        for (bytes, max, want) in CASES {
            let got = read(Method::Get, bytes, *max).await.expect_err("refused");
            assert_eq!(got, *want, "{}", String::from_utf8_lossy(bytes));
        }
    }

    #[tokio::test]
    async fn a_stream_that_ends_before_the_head_is_unreachable() {
        let (mut client, peer) = duplex(64);
        drop(peer);
        let got = read_response(&mut client, Method::Get, 10)
            .await
            .expect_err("eof");
        assert_eq!(got, HttpError::Unreachable);
    }

    /// A stream that says `sent` and then stops answering: its owner's idle limit, as
    /// `TimedOut`, is what a read finds next.
    struct GoesQuiet {
        sent: Vec<u8>,
    }

    impl ByteStream for GoesQuiet {
        async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.sent.is_empty() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let n = self.sent.len().min(buf.len());
            buf[..n].copy_from_slice(&self.sent[..n]);
            self.sent.drain(..n);
            Ok(n)
        }

        async fn write_all(&mut self, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }

        async fn shutdown(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_server_that_stops_answering_is_timed_out_in_the_head_and_in_every_kind_of_body() {
        const CASES: &[&[u8]] = &[
            // Nothing at all.
            b"",
            // Half a head.
            b"HTTP/1.1 207 Multi-Status\r\nContent-Le",
            // A length it never finishes.
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc",
            // Chunks it never finishes.
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n",
            // A body that runs to the end of the stream: a stall is not that end, so the part
            // read is never taken for the whole answer.
            b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabc",
        ];
        for bytes in CASES {
            let mut stream = GoesQuiet {
                sent: bytes.to_vec(),
            };
            let got = read_response(&mut stream, Method::Get, 100)
                .await
                .expect_err("a stall");
            assert_eq!(
                got,
                HttpError::TimedOut,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn only_a_timed_out_stream_is_a_timeout() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            io_fault(&Error::from(ErrorKind::TimedOut)),
            HttpError::TimedOut
        );
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionReset,
            ErrorKind::UnexpectedEof,
        ] {
            assert_eq!(
                io_fault(&Error::from(kind)),
                HttpError::Unreachable,
                "{kind:?}"
            );
        }
    }
}
