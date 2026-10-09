//! The LocalAPI over tailscaled's unix socket (feature `io`).
//!
//! The requests are HTTP/1.1 on the socket, made with hyper's own client; the `Host` is the
//! `local-tailscaled.sock` Tailscale's server expects (`apitype.LocalAPIHost`), and the user is
//! known to tailscaled by the socket's peer credentials, so nothing is sent as a password. The
//! default socket path is only a default: every test names its own.
//!
//! Endpoints, as of v1.80.0 (`ipn/localapi/localapi.go`): `GET status`, `GET whois?addr=`,
//! `POST login-interactive` (204; the page to open arrives as `BrowseToURL` on the watch stream
//! and as `AuthURL` in `status`) and `GET watch-ipn-bus?mask=` (newline-delimited `Notify`,
//! flushed per notice, until the request ends).

use crate::error::TailscaleError;
use crate::notice::Notice;
use crate::status::Status;
use crate::whois::WhoIs;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::UnixStream;

/// Where tailscaled serves its LocalAPI on Linux.
pub const DEFAULT_SOCKET: &str = "/var/run/tailscale/tailscaled.sock";

/// The `Host` header tailscaled's LocalAPI accepts (`apitype.LocalAPIHost`).
const LOCALAPI_HOST: &str = "local-tailscaled.sock";

/// The most of an answer that is read. A status of a few thousand computers is a few megabytes.
const MAX_ANSWER: usize = 16 * 1024 * 1024;

/// The most of one line of the watch stream that is read.
const MAX_LINE: usize = 16 * 1024 * 1024;

/// What the watch asks tailscaled to send: the state and the net map when it starts
/// (`NotifyInitialState` 2, `NotifyInitialNetMap` 8), no private keys (`NotifyNoPrivateKeys`
/// 16, without which the stream needs write access), and at most a few notices a second
/// (`NotifyRateLimit` 256).
const WATCH_MASK: u32 = 2 | 8 | 16 | 256;

/// The places a Tailscale daemon's program is looked for when its socket is missing, besides the
/// directories of `PATH`: if it is there, Tailscale is installed and just not running.
const PROGRAM_DIRS: [&str; 5] = [
    "/usr/sbin",
    "/usr/bin",
    "/usr/local/sbin",
    "/usr/local/bin",
    "/opt/tailscale",
];

/// The name of the program that serves the LocalAPI.
const PROGRAM: &str = "tailscaled";

/// A client of one tailscaled's LocalAPI.
#[derive(Debug, Clone)]
pub struct LocalApi {
    socket: PathBuf,
    program_dirs: Vec<PathBuf>,
    timeout: Duration,
}

impl LocalApi {
    /// A client of the LocalAPI at `socket`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default();
        Self {
            socket: socket.into(),
            program_dirs: path_dirs
                .into_iter()
                .chain(PROGRAM_DIRS.iter().map(PathBuf::from))
                .collect(),
            timeout: Duration::from_secs(10),
        }
    }

    /// A client of this computer's own Tailscale, at the path it uses.
    pub fn system() -> Self {
        Self::new(DEFAULT_SOCKET)
    }

    /// The same client looking for Tailscale's program only in `dirs` (to tell "not installed"
    /// from "not running" when the socket is missing).
    pub fn with_program_dirs(self, dirs: Vec<PathBuf>) -> Self {
        Self {
            program_dirs: dirs,
            ..self
        }
    }

    /// The same client giving up on a request after `timeout`.
    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// The socket this client dials.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn installed(&self) -> bool {
        self.program_dirs
            .iter()
            .any(|dir| dir.join(PROGRAM).is_file())
    }

    /// What a failed dial of the socket comes to.
    fn refused_by(&self, kind: ErrorKind) -> TailscaleError {
        match kind {
            ErrorKind::NotFound => match self.installed() {
                true => TailscaleError::NotRunning,
                false => TailscaleError::NotInstalled,
            },
            ErrorKind::PermissionDenied => TailscaleError::Refused,
            _ => TailscaleError::NotRunning,
        }
    }

    /// Sends `method target` and returns the response, its body not read yet.
    async fn send(
        &self,
        method: Method,
        target: &str,
    ) -> Result<hyper::Response<Incoming>, TailscaleError> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|e| self.refused_by(e.kind()))?;
        let (mut sender, connection) = http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream))
            .await
            .map_err(|_| TailscaleError::NotRunning)?;
        // The connection ends when the response and the sender are dropped, or tailscaled stops.
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = Request::builder()
            .method(method)
            .uri(target)
            .header(hyper::header::HOST, LOCALAPI_HOST)
            .body(Full::new(Bytes::new()))
            .map_err(|_| TailscaleError::Malformed)?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| TailscaleError::NotRunning)?;
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(TailscaleError::Refused),
            _ => Ok(response),
        }
    }

    /// `method target` to its whole body, within the timeout.
    async fn exchange(&self, method: Method, target: &str) -> Result<Answer, TailscaleError> {
        let work = async {
            let response = self.send(method, target).await?;
            let status = response.status();
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| TailscaleError::NotRunning)?;
                if let Some(data) = frame.data_ref() {
                    if bytes.len() + data.len() > MAX_ANSWER {
                        return Err(TailscaleError::Malformed);
                    }
                    bytes.extend_from_slice(data);
                }
            }
            Ok(Answer {
                status,
                body: bytes,
            })
        };
        tokio::time::timeout(self.timeout, work)
            .await
            .unwrap_or(Err(TailscaleError::TimedOut))
    }

    /// This computer's Tailscale, as it says it is now. Signed out is an answer, not an error:
    /// read [`Status::standing`].
    pub async fn status(&self) -> Result<Status, TailscaleError> {
        let answer = self.exchange(Method::GET, "/localapi/v0/status").await?;
        match answer.status {
            StatusCode::OK => Status::parse(&answer.body),
            _ => Err(TailscaleError::Malformed),
        }
    }

    /// Who the address `addr` (a Tailscale address and the port it dialled from) belongs to.
    /// An address no computer of the network has is [`TailscaleError::NoSuchPeer`].
    pub async fn whois(&self, addr: SocketAddr) -> Result<WhoIs, TailscaleError> {
        let target = format!("/localapi/v0/whois?addr={}", encode_addr(&addr.to_string()));
        let answer = self.exchange(Method::GET, &target).await?;
        match answer.status {
            StatusCode::OK => WhoIs::parse(&answer.body),
            StatusCode::NOT_FOUND => Err(TailscaleError::NoSuchPeer),
            _ => Err(TailscaleError::Malformed),
        }
    }

    /// Asks Tailscale to start its own sign-in (`login-interactive`). The page the person opens
    /// is Tailscale's: read it from [`Status::auth_url`] once it has one. Needs write access to
    /// the socket, which Tailscale gives its operator.
    pub async fn start_login(&self) -> Result<(), TailscaleError> {
        let answer = self
            .exchange(Method::POST, "/localapi/v0/login-interactive")
            .await?;
        match answer.status.is_success() {
            true => Ok(()),
            false => Err(TailscaleError::Malformed),
        }
    }

    /// Starts watching Tailscale's notices. The first answer comes within the timeout; the
    /// stream after it stays open until it is dropped or Tailscale stops.
    pub async fn watch(&self) -> Result<Watch, TailscaleError> {
        let target = format!("/localapi/v0/watch-ipn-bus?mask={WATCH_MASK}");
        let response = tokio::time::timeout(self.timeout, self.send(Method::GET, &target))
            .await
            .unwrap_or(Err(TailscaleError::TimedOut))?;
        match response.status() {
            StatusCode::OK => Ok(Watch {
                body: response.into_body(),
                buffer: Vec::new(),
            }),
            _ => Err(TailscaleError::Malformed),
        }
    }
}

struct Answer {
    status: StatusCode,
    body: Vec<u8>,
}

/// `addr` with the characters a query cannot hold written as `%XX`.
fn encode_addr(addr: &str) -> String {
    addr.chars()
        .map(|c| match c {
            ':' => "%3A".to_owned(),
            '[' => "%5B".to_owned(),
            ']' => "%5D".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

/// Tailscale's notices, as they arrive.
#[derive(Debug)]
pub struct Watch {
    body: Incoming,
    buffer: Vec<u8>,
}

impl Watch {
    /// The next notice. `None` when Tailscale ended the stream; an error when it went away
    /// mid-line or said something that is not a notice.
    pub async fn next(&mut self) -> Result<Option<Notice>, TailscaleError> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                return Notice::parse(&line).map(Some);
            }
            match self.body.frame().await {
                None => return Ok(None),
                Some(Err(_)) => return Err(TailscaleError::NotRunning),
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        if self.buffer.len() + data.len() > MAX_LINE {
                            return Err(TailscaleError::Malformed);
                        }
                        self.buffer.extend_from_slice(data);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_written_for_a_query() {
        assert_eq!(encode_addr("100.64.0.2:5000"), "100.64.0.2%3A5000");
        assert_eq!(encode_addr("[fd7a::2]:5000"), "%5Bfd7a%3A%3A2%5D%3A5000");
    }

    #[test]
    fn the_watch_mask_asks_for_no_private_keys() {
        // Without NotifyNoPrivateKeys (16) tailscaled wants write access for the stream.
        assert_eq!(WATCH_MASK & 16, 16);
        assert_eq!(WATCH_MASK, 282);
    }
}
