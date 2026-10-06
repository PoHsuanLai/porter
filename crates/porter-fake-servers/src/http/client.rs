//! The minimal client the fakes' self-tests and the scripted browser use, and the record of what
//! a fake answered.

use super::*;

/// How a client reaches a fake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// Plain HTTP.
    Http,
    /// HTTPS, trusting the scratch CA.
    Https,
}

/// Sends one request to a fake and reads the whole response (the test's minimal client).
pub async fn send(
    address: &FakeAddress,
    scheme: Scheme,
    request: &Request,
) -> io::Result<Response> {
    let conn = dial(address).await?;
    match scheme {
        Scheme::Http => exchange(conn, request).await,
        Scheme::Https => {
            exchange(
                tls::connector().connect(tls::server_name(), conn).await?,
                request,
            )
            .await
        }
    }
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    request: &Request,
) -> io::Result<Response> {
    let mut stream = stream;
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n",
        request.method, request.target
    );
    for (n, v) in &request.headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", request.body.len()));
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&request.body).await?;
    stream.flush().await?;
    let mut reader = BufReader::new(stream);
    let (status_line, headers) = read_head(&mut reader)
        .await?
        .ok_or(io::ErrorKind::UnexpectedEof)?;
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or(io::ErrorKind::InvalidData)?;
    let body = read_body(&mut reader, &headers).await?;
    Ok(Response {
        status,
        headers,
        body,
    })
}

/// A plain-HTTP GET (the test's minimal client).
pub async fn get(address: &FakeAddress, target: &str) -> io::Result<Response> {
    send(address, Scheme::Http, &Request::new("GET", target)).await
}

/// One request a fake answered, as recorded for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The method.
    pub method: String,
    /// Path and query.
    pub target: String,
    /// The `Authorization` header exactly as received, if any.
    pub authorization: Option<String>,
    /// The `If-Match` header as received, if any.
    pub if_match: Option<String>,
    /// The `If-None-Match` header as received, if any.
    pub if_none_match: Option<String>,
    /// The `Range` header as received, if any.
    pub range: Option<String>,
    /// The status sent back.
    pub status: u16,
}

impl Hit {
    /// What was asked and answered.
    pub fn of(request: &Request, response: &Response) -> Self {
        Self {
            method: request.method.clone(),
            target: request.target.clone(),
            authorization: request.header("authorization").map(str::to_owned),
            if_match: request.header("if-match").map(str::to_owned),
            if_none_match: request.header("if-none-match").map(str::to_owned),
            range: request.header("range").map(str::to_owned),
            status: response.status,
        }
    }
}

/// A plain-HTTP form POST (the test's minimal client).
pub async fn post_form(
    address: &FakeAddress,
    target: &str,
    pairs: &[(&str, &str)],
) -> io::Result<Response> {
    let body = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let request = Request::new("POST", target)
        .with_header("Content-Type", "application/x-www-form-urlencoded")
        .with_body(body);
    send(address, Scheme::Http, &request).await
}
