//! [`RelayHttp`]: hyper's HTTP/1 connection over the streams a [`Dial`] opens.

use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Bytes, Incoming};
use hyper::client::conn::http1::{self, SendRequest};
use hyper_util::rt::TokioIo;
use porter_http::HttpError;
use porter_http::http::header::{ACCEPT_ENCODING, HOST, HeaderValue};
use porter_http::http::{Request, Response, StatusCode, Uri};
use std::error::Error;
use std::future::Future;
use std::io;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Instant;

/// How long a relay may stay silent, once a request is waiting on it, before the exchange fails:
/// the wait for a response head, and the wait between two pieces of a body. A server that stops
/// answering (a half-open connection the far end never closes) would otherwise stall the dataset
/// forever.
pub const RELAY_IDLE: Duration = Duration::from_secs(60);

type BoxError = Box<dyn Error + Send + Sync>;
type Outgoing = UnsyncBoxBody<Bytes, BoxError>;

/// Opens connections to the endpoint, each already authenticated.
pub trait Dial: Send + Sync {
    /// What a connection reads and writes.
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// A new connection.
    fn dial(&self) -> impl Future<Output = Result<Self::Stream, HttpError>> + Send;
}

/// How [`RelayHttp`] behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayLimits {
    /// The most a response body may hold when it is read whole ([`Http::send`](porter_http::Http)).
    /// A streamed response ([`RelayHttp::send_streaming`]) has no cap.
    pub max_body: usize,
    /// How many idle connections are kept for the next request.
    pub idle: usize,
    /// How long a connection may be silent while a request waits on it: no byte moved in either
    /// direction for this long is `TimedOut`. A transfer that keeps moving is never cut short.
    pub silence: Duration,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            max_body: 256 * 1024 * 1024,
            idle: 2,
            silence: RELAY_IDLE,
        }
    }
}

/// When a connection last moved a byte.
#[derive(Debug)]
struct Progress(Mutex<Instant>);

impl Progress {
    fn now() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Instant::now())))
    }

    fn at(&self) -> MutexGuard<'_, Instant> {
        // A lock around one assignment.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn touch(&self) {
        *self.at() = Instant::now();
    }

    fn quiet(&self) -> Duration {
        self.at().elapsed()
    }
}

/// `work`, or `TimedOut` once the connection has been silent for `silence`.
async fn within<T>(
    progress: &Progress,
    silence: Duration,
    work: impl Future<Output = T>,
) -> Result<T, HttpError> {
    let mut work = pin!(work);
    loop {
        let wait = silence.saturating_sub(progress.quiet());
        match tokio::time::timeout(wait, work.as_mut()).await {
            Ok(done) => return Ok(done),
            Err(_) if progress.quiet() >= silence => return Err(HttpError::TimedOut),
            Err(_) => {}
        }
    }
}

/// A stream that notes every byte it moves.
#[derive(Debug)]
struct Watched<S> {
    inner: S,
    progress: Arc<Progress>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Watched<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(polled, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.progress.touch();
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Watched<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if matches!(polled, Poll::Ready(Ok(n)) if n > 0) {
            self.progress.touch();
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// One open connection: the handle that sends on it (its driving task runs on the runtime).
#[derive(Debug)]
struct Conn {
    sender: SendRequest<Outgoing>,
    progress: Arc<Progress>,
}

/// The connections kept for the next request.
#[derive(Debug)]
struct Pool {
    kept: Mutex<Vec<Conn>>,
    most: usize,
}

impl Pool {
    fn kept(&self) -> MutexGuard<'_, Vec<Conn>> {
        // Every critical section is a push, a pop or a clear.
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn take(&self) -> Option<Conn> {
        self.kept().pop()
    }

    fn give(&self, conn: Conn) {
        let mut kept = self.kept();
        if kept.len() < self.most {
            kept.push(conn);
        }
    }

    fn clear(&self) {
        self.kept().clear();
    }
}

/// An [`Http`](porter_http::Http) over a [`Dial`]: connections are reused while the server keeps
/// them open, and a request that finds a kept connection dead, or answered `401` (its credential
/// lapsed while it sat idle), is sent once more on a new one. A streamed request body is never
/// sent twice, so [`send_streaming`](Self::send_streaming) always opens a new connection.
pub struct RelayHttp<D> {
    dial: D,
    limits: RelayLimits,
    pool: Arc<Pool>,
}

impl<D> std::fmt::Debug for RelayHttp<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayHttp")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl<D: Dial> RelayHttp<D> {
    /// A client dialling with `dial`.
    pub fn new(dial: D, limits: RelayLimits) -> Self {
        Self {
            dial,
            limits,
            pool: Arc::new(Pool {
                kept: Mutex::new(Vec::new()),
                most: limits.idle,
            }),
        }
    }

    /// Sends `request` on a new connection, its body as `body` produces it, and returns the
    /// response with its body still to be read from the [`RelayBody`]. Neither the request nor the
    /// response is held whole, and neither has a size cap; a body of known length says so in its
    /// size hint or a `Content-Length` header, and otherwise goes out chunked.
    ///
    /// # Errors
    /// [`HttpError`] when no connection opened or no response head arrived.
    pub async fn send_streaming<B>(
        &self,
        request: Request<B>,
    ) -> Result<Response<RelayBody>, HttpError>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let request = prepare(request.map(|body| body.map_err(Into::into).boxed_unsync()))?;
        let mut conn = self.open().await?;
        let response = self.exchange(&mut conn, request).await?;
        let (head, incoming) = response.into_parts();
        Ok(Response::from_parts(head, self.body(incoming, conn)))
    }

    fn body(&self, incoming: Incoming, conn: Conn) -> RelayBody {
        RelayBody {
            incoming: Some(incoming),
            conn: Some(conn),
            pool: Arc::clone(&self.pool),
            silence: self.limits.silence,
        }
    }

    async fn open(&self) -> Result<Conn, HttpError> {
        let stream = self.dial.dial().await?;
        let progress = Progress::now();
        let io = TokioIo::new(Watched {
            inner: stream,
            progress: Arc::clone(&progress),
        });
        let (sender, connection) = within(&progress, self.limits.silence, http1::handshake(io))
            .await?
            .map_err(fault)?;
        tokio::spawn(async move {
            // The connection ends when the server closes it or the sender is dropped.
            let _ = connection.await;
        });
        Ok(Conn { sender, progress })
    }

    async fn exchange(
        &self,
        conn: &mut Conn,
        request: Request<Outgoing>,
    ) -> Result<Response<Incoming>, HttpError> {
        conn.progress.touch();
        within(
            &conn.progress,
            self.limits.silence,
            conn.sender.send_request(request),
        )
        .await?
        .map_err(fault)
    }

    /// The whole response, read from `incoming` within the cap.
    async fn whole(
        &self,
        response: Response<Incoming>,
        conn: Conn,
    ) -> Result<Response<Vec<u8>>, HttpError> {
        let (head, incoming) = response.into_parts();
        let mut body = self.body(incoming, conn);
        let mut bytes = Vec::new();
        while let Some(piece) = body.next().await? {
            if bytes.len() + piece.len() > self.limits.max_body {
                return Err(HttpError::TooLarge);
            }
            bytes.extend_from_slice(&piece);
        }
        Ok(Response::from_parts(head, bytes))
    }
}

impl<D: Dial> porter_http::Http for RelayHttp<D> {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
        let (parts, body) = request.into_parts();
        let body = Bytes::from(body);
        let again = || {
            let body = Full::new(body.clone())
                .map_err(|never| -> BoxError { match never {} })
                .boxed_unsync();
            prepare(Request::from_parts(parts.clone(), body))
        };
        if let Some(mut conn) = self.pool.take() {
            // A connection that is gone is one the server closed while it sat idle; any other
            // failure is the server's answer and is not sent twice.
            match self.exchange(&mut conn, again()?).await {
                // A kept connection carries the credential it was opened with, which may have
                // lapsed while it sat idle. Every kept connection holds the same one, so none is
                // used again; the request goes out once more on a new connection, whose dial
                // presents a fresh credential (or is refused, which the dial reports).
                Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                    self.pool.clear();
                }
                Ok(response) => return self.whole(response, conn).await,
                Err(HttpError::Unreachable) => {}
                Err(other) => return Err(other),
            }
        }
        let mut conn = self.open().await?;
        let response = self.exchange(&mut conn, again()?).await?;
        self.whole(response, conn).await
    }
}

/// A response body, read piece by piece. A body read to its end hands its connection back for
/// the next request; one dropped before that closes it.
#[derive(Debug)]
pub struct RelayBody {
    incoming: Option<Incoming>,
    conn: Option<Conn>,
    pool: Arc<Pool>,
    silence: Duration,
}

impl RelayBody {
    /// The next piece of the body, or `None` at its end. A connection silent for the limits'
    /// `silence` is `TimedOut`; trailers are skipped.
    ///
    /// # Errors
    /// [`HttpError`] when the body breaks off.
    pub async fn next(&mut self) -> Result<Option<Bytes>, HttpError> {
        loop {
            let (Some(incoming), Some(conn)) = (self.incoming.as_mut(), self.conn.as_ref()) else {
                return Ok(None);
            };
            let frame = within(&conn.progress, self.silence, incoming.frame()).await;
            match frame {
                Err(timed_out) => return Err(self.broken(timed_out)),
                Ok(Some(Err(error))) => return Err(self.broken(fault(error))),
                Ok(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        return Ok(Some(data));
                    }
                }
                Ok(None) => {
                    self.finish().await;
                    return Ok(None);
                }
            }
        }
    }

    /// The body ended: the connection goes back if the server keeps it open.
    async fn finish(&mut self) {
        self.incoming = None;
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        let ready = within(&conn.progress, self.silence, conn.sender.ready()).await;
        if matches!(ready, Ok(Ok(()))) {
            self.pool.give(conn);
        }
    }

    /// The connection is not kept after a failure.
    fn broken(&mut self, error: HttpError) -> HttpError {
        self.incoming = None;
        self.conn = None;
        error
    }
}

/// The request as it is written on the wire: origin-form target, a `Host` and no compression.
fn prepare(request: Request<Outgoing>) -> Result<Request<Outgoing>, HttpError> {
    let (mut parts, body) = request.into_parts();
    let authority = parts.uri.authority().map(|a| a.as_str().to_owned());
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", |target| target.as_str());
    parts.uri = target.parse::<Uri>().map_err(|_| HttpError::Malformed)?;
    if let (false, Some(authority)) = (parts.headers.contains_key(HOST), authority) {
        let host = HeaderValue::from_str(&authority).map_err(|_| HttpError::Malformed)?;
        parts.headers.insert(HOST, host);
    }
    parts
        .headers
        .entry(ACCEPT_ENCODING)
        .or_insert(HeaderValue::from_static("identity"));
    Ok(Request::from_parts(parts, body))
}

/// What a hyper error comes to: a connection that is gone, a stall, or garbage.
fn fault(error: hyper::Error) -> HttpError {
    if error.is_timeout() {
        return HttpError::TimedOut;
    }
    if error.is_canceled() || error.is_closed() || error.is_incomplete_message() {
        return HttpError::Unreachable;
    }
    let mut current = error.source();
    while let Some(source) = current {
        if let Some(io) = source.downcast_ref::<io::Error>() {
            return match io.kind() {
                io::ErrorKind::TimedOut => HttpError::TimedOut,
                io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::NotConnected
                | io::ErrorKind::UnexpectedEof => HttpError::Unreachable,
                _ => HttpError::Malformed,
            };
        }
        current = source.source();
    }
    HttpError::Malformed
}
