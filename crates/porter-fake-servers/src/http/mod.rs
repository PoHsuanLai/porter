//! A small HTTP/1.1 server and client for the fakes: one request in, one response out, bodies by
//! `Content-Length`. The fakes' handlers are plain functions over `Request`, so what a fake
//! answers is a table a test can read.

mod client;

pub use client::*;

use crate::net::{Listener, dial};
use crate::tls;
use porter_fake::FakeAddress;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_rustls::TlsAcceptor;

/// One request as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET`, `PROPFIND`, ...
    pub method: String,
    /// The request target: path and query.
    pub target: String,
    /// Headers in arrival order, names as sent.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

/// One response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// Headers (`Content-Length` is added on the wire).
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Request {
    /// A request with no headers or body.
    pub fn new(method: &str, target: &str) -> Self {
        Self {
            method: method.to_owned(),
            target: target.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// With a header.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// With a body.
    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// With HTTP Basic credentials.
    pub fn with_basic(self, user: &str, password: &str) -> Self {
        use base64::Engine;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        self.with_header("Authorization", &format!("Basic {token}"))
    }

    /// The path without the query.
    pub fn path(&self) -> &str {
        self.target.split_once('?').map_or(&self.target, |(p, _)| p)
    }

    /// The decoded query pairs.
    pub fn query(&self) -> Vec<(String, String)> {
        self.target
            .split_once('?')
            .map_or_else(Vec::new, |(_, q)| form_pairs(q))
    }

    /// One query value.
    pub fn query_value(&self, name: &str) -> Option<String> {
        pair(&self.query(), name)
    }

    /// The body as form-urlencoded pairs.
    pub fn form(&self) -> Vec<(String, String)> {
        form_pairs(&self.body_text())
    }

    /// One form value.
    pub fn form_value(&self, name: &str) -> Option<String> {
        pair(&self.form(), name)
    }

    /// The body as text.
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// A header by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        header_of(&self.headers, name)
    }

    /// The user and password of a Basic `Authorization`.
    pub fn basic(&self) -> Option<(String, String)> {
        use base64::Engine;
        let value = self.header("authorization")?.strip_prefix("Basic ")?;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .ok()?;
        let text = String::from_utf8(raw).ok()?;
        let (user, password) = text.split_once(':')?;
        Some((user.to_owned(), password.to_owned()))
    }

    /// The token of a Bearer `Authorization`.
    pub fn bearer(&self) -> Option<&str> {
        self.header("authorization")?.strip_prefix("Bearer ")
    }
}

impl Response {
    /// An empty response.
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// A JSON response.
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self::new(status).typed("application/json", value.to_string())
    }

    /// A response with a content type and body.
    pub fn typed(mut self, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        self.headers
            .push(("Content-Type".to_owned(), content_type.to_owned()));
        self.body = body.into();
        self
    }

    /// A `302` to `location`.
    pub fn redirect(location: &str) -> Self {
        Self::new(302).with_header("Location", location)
    }

    /// With a header.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// A header by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        header_of(&self.headers, name)
    }

    /// The body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body as JSON.
    pub fn json_body(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

fn header_of<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn pair(pairs: &[(String, String)], name: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

/// `a=1&b=two%20words` as decoded pairs.
pub fn form_pairs(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

/// Percent-decodes (and reads `+` as a space).
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len()
                && hex(bytes[i + 1]).is_some()
                && hex(bytes[i + 2]).is_some() =>
            {
                out.push(
                    (hex(bytes[i + 1]).unwrap_or(0) * 16 + hex(bytes[i + 2]).unwrap_or(0)) as u8,
                );
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes everything but unreserved characters.
pub fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn read_head<S: AsyncRead + Unpin>(
    reader: &mut BufReader<S>,
) -> io::Result<Option<(String, Vec<(String, String)>)>> {
    let mut first = String::new();
    if reader.read_line(&mut first).await? == 0 {
        return Ok(None);
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((n, v)) = line.split_once(':') {
            headers.push((n.trim().to_owned(), v.trim().to_owned()));
        }
    }
    Ok(Some((first.trim_end().to_owned(), headers)))
}

async fn read_body<S: AsyncRead + Unpin>(
    reader: &mut BufReader<S>,
    headers: &[(String, String)],
) -> io::Result<Vec<u8>> {
    let length = header_of(headers, "content-length").and_then(|v| v.parse::<usize>().ok());
    let mut body = Vec::new();
    match length {
        Some(n) => {
            body.resize(n, 0);
            reader.read_exact(&mut body).await?;
        }
        None => {
            reader.read_to_end(&mut body).await?;
        }
    }
    Ok(body)
}

async fn read_request<S: AsyncRead + Unpin>(
    reader: &mut BufReader<S>,
) -> io::Result<Option<Request>> {
    let Some((first, headers)) = read_head(reader).await? else {
        return Ok(None);
    };
    let mut parts = first.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let has_body = header_of(&headers, "content-length").is_some();
    let body = if has_body {
        read_body(reader, &headers).await?
    } else {
        Vec::new()
    };
    Ok(Some(Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body,
    }))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        412 => "Precondition Failed",
        416 => "Range Not Satisfiable",
        501 => "Not Implemented",
        507 => "Insufficient Storage",
        _ => "Status",
    }
}

async fn write_response<S: AsyncWrite + Unpin>(
    stream: &mut S,
    response: &Response,
    close: bool,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (n, v) in &response.headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    let connection = if close { "close" } else { "keep-alive" };
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: {connection}\r\n\r\n",
        response.body.len()
    ));
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await
}

async fn serve_stream<S, H>(stream: S, handler: &H) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Fn(Request) -> Response,
{
    let mut reader = BufReader::new(stream);
    while let Some(request) = read_request(&mut reader).await? {
        let close = request
            .header("connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"));
        let response = handler(request);
        write_response(reader.get_mut(), &response, close).await?;
        if close {
            break;
        }
    }
    Ok(())
}

/// Accepts connections and answers each request with `handler`, over TLS when `tls` is given.
/// Runs until the future is dropped, which also ends every connection it is serving (a stopped
/// fake does not go on answering a client that kept its connection open).
pub async fn serve<H>(listener: Listener, tls: Option<TlsAcceptor>, handler: Arc<H>)
where
    H: Fn(Request) -> Response + Send + Sync + 'static,
{
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((conn, _peer)) = accepted else { return };
                let (handler, tls) = (Arc::clone(&handler), tls.clone());
                connections.spawn(async move {
                    // A connection that fails mid-request is a client that went away.
                    let _ = match tls {
                        Some(acceptor) => match acceptor.accept(conn).await {
                            Ok(stream) => serve_stream(stream, &*handler).await,
                            Err(e) => Err(e),
                        },
                        None => serve_stream(conn, &*handler).await,
                    };
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}
