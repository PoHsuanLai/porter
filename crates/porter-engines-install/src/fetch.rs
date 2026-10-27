//! One pinned file from an `https` address to a path: resumable, cancellable, counted, and
//! kept only if its size and SHA-256 are the pinned ones.
//!
//! The bytes go to `<dest>.part` beside the destination. A broken connection is picked up again
//! with a `Range` request from the bytes already there (the file is hashed first, so the
//! checksum covers all of it). A file that is complete but wrong is deleted. A stop or a lost
//! connection that outlasts the retries leaves the part for the next try.

use crate::progress::{CancelToken, Meter};
use porter_core::{UrlScheme, WebUrl};
use porter_engines::{Artifact, file_digest, hex};
use porter_http::http::header::RANGE;
use porter_http::http::{HeaderMap, Method, StatusCode};
use porter_http::{HeaderText, HttpError, request};
use porter_http_reqwest::{BodySink, Flow, ReqwestHttp};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How many redirects one download follows.
const MAX_REDIRECTS: usize = 5;

/// Why a download did not produce its file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// The connection failed or broke off, and the retries ran out.
    #[error("the download could not be finished: {0}")]
    Network(HttpError),
    /// The server answered with this status.
    #[error("the server answered {0}")]
    Refused(u16),
    /// What arrived is not what is pinned; it was deleted.
    #[error("what arrived is not what is pinned")]
    Corrupt,
    /// The disk refused the bytes.
    #[error("the disk refused the download: {0}")]
    Disk(std::io::ErrorKind),
    /// A stop was asked for.
    #[error("stopped")]
    Cancelled,
    /// An address that is not `https`, or redirects that lead nowhere.
    #[error("the address is not an https one, or the redirects lead nowhere")]
    BadAddress,
}

/// Downloads pinned files.
#[derive(Debug, Clone)]
pub struct Fetcher {
    http: ReqwestHttp,
    idle: Duration,
    retries: u32,
    pause: Duration,
}

impl Fetcher {
    /// A fetcher that trusts the system's certificate authorities, with the usual waits.
    pub fn system() -> Self {
        Self::new(ReqwestHttp::new())
    }

    /// A fetcher over `http`: 30 seconds of silence is a lost connection, and a lost connection
    /// is retried three times, two seconds apart.
    pub fn new(http: ReqwestHttp) -> Self {
        Self {
            http,
            idle: Duration::from_secs(30),
            retries: 3,
            pause: Duration::from_secs(2),
        }
    }

    /// The same, with these waits and this many retries (a test's own).
    #[must_use]
    pub fn with_timing(self, idle: Duration, retries: u32, pause: Duration) -> Self {
        Self {
            idle,
            retries,
            pause,
            ..self
        }
    }

    /// Brings `artifact` to `dest`. A `dest` that already holds the pinned file is kept as it is.
    /// `meter` gains the file's bytes as they arrive.
    ///
    /// # Errors
    /// See [`FetchError`].
    pub async fn fetch(
        &self,
        artifact: &Artifact,
        dest: &Path,
        meter: &Meter,
        cancel: &CancelToken,
    ) -> Result<(), FetchError> {
        let base = meter.done();
        if file_digest(dest)
            .is_ok_and(|(size, hash)| size == artifact.size && hash == artifact.sha256)
        {
            meter.set_done(base + artifact.size);
            return Ok(());
        }
        let _stale = std::fs::remove_file(dest);
        let part = part_of(dest);
        let mut attempt = 0;
        loop {
            match self.once(artifact, &part, base, meter, cancel).await {
                Ok(()) => break,
                Err(FetchError::Network(_)) if attempt < self.retries && !cancel.is_cancelled() => {
                    attempt += 1;
                    tokio::select! {
                        () = tokio::time::sleep(self.pause) => {}
                        () = cancel.cancelled() => return Err(FetchError::Cancelled),
                    }
                }
                Err(other) => return Err(other),
            }
        }
        std::fs::rename(&part, dest).map_err(|e| FetchError::Disk(e.kind()))
    }

    async fn once(
        &self,
        artifact: &Artifact,
        part: &Path,
        base: u64,
        meter: &Meter,
        cancel: &CancelToken,
    ) -> Result<(), FetchError> {
        let mut url = artifact.url.to_owned();
        for _hop in 0..=MAX_REDIRECTS {
            let mut sink = Sink::resume(part, artifact.size, base, meter, cancel)?;
            let web = WebUrl::parse(&url).map_err(|_| FetchError::BadAddress)?;
            if web.origin().scheme != UrlScheme::Https {
                return Err(FetchError::BadAddress);
            }
            let mut asked = request(Method::GET, &web);
            if sink.have > 0 {
                asked = asked.header(RANGE, format!("bytes={}-", sink.have));
            }
            let asked = asked
                .body(Vec::new())
                .map_err(|e| FetchError::Network(HttpError::from(e)))?;
            let sent = tokio::select! {
                () = cancel.cancelled() => None,
                sent = self.http.stream(asked, self.idle, &mut sink) => Some(sent),
            };
            let Some(sent) = sent else {
                return Err(FetchError::Cancelled);
            };
            let status = sent.map_err(FetchError::Network)?;
            match (status.as_u16(), sink.redirect.take()) {
                (301 | 302 | 303 | 307 | 308, Some(location)) => {
                    url = resolve(&web, &location)?;
                }
                (200 | 206, _) => return sink.complete(artifact, part),
                (416, _) => {
                    // The part is longer than the file or not a part of it: start again.
                    let _gone = std::fs::remove_file(part);
                    meter.set_done(base);
                }
                (other, _) => return Err(FetchError::Refused(other)),
            }
        }
        Err(FetchError::BadAddress)
    }
}

/// `dest` with `.part` after its name.
fn part_of(dest: &Path) -> PathBuf {
    let mut name: OsString = dest.file_name().map(OsString::from).unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

/// A `Location` as an absolute `https` address: absolute already, or a path on the same host.
fn resolve(from: &WebUrl, location: &str) -> Result<String, FetchError> {
    if location.starts_with("https://") {
        Ok(location.to_owned())
    } else if location.starts_with('/') {
        let origin = from.origin();
        Ok(format!("https://{}:{}{location}", origin.host, origin.port))
    } else {
        Err(FetchError::BadAddress)
    }
}

/// Receives one response into the part file.
struct Sink {
    file: File,
    hasher: Sha256,
    /// Bytes in the file now.
    have: u64,
    expect: u64,
    base: u64,
    meter: Meter,
    cancel: CancelToken,
    writing: bool,
    redirect: Option<String>,
    failed: Option<std::io::ErrorKind>,
    overrun: bool,
    stopped: bool,
    misplaced: bool,
}

impl Sink {
    /// Opens the part file (making it) and hashes what is already in it; a part longer than the
    /// file is emptied.
    fn resume(
        part: &Path,
        expect: u64,
        base: u64,
        meter: &Meter,
        cancel: &CancelToken,
    ) -> Result<Self, FetchError> {
        let disk = |e: std::io::Error| FetchError::Disk(e.kind());
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(part)
            .map_err(disk)?;
        let mut sink = Self {
            hasher: Sha256::new(),
            have: 0,
            file: file.try_clone().map_err(disk)?,
            expect,
            base,
            meter: meter.clone(),
            cancel: cancel.clone(),
            writing: false,
            redirect: None,
            failed: None,
            overrun: false,
            stopped: false,
            misplaced: false,
        };
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(disk)?;
            if read == 0 {
                break;
            }
            sink.hasher.update(&buffer[..read]);
            sink.have += read as u64;
        }
        if sink.have > expect {
            sink.start_over()?;
        }
        meter.set_done(base + sink.have);
        Ok(sink)
    }

    fn start_over(&mut self) -> Result<(), FetchError> {
        self.file
            .set_len(0)
            .map_err(|e| FetchError::Disk(e.kind()))?;
        self.hasher = Sha256::new();
        self.have = 0;
        self.meter.set_done(self.base);
        Ok(())
    }

    /// What the finished response comes to.
    fn complete(self, artifact: &Artifact, part: &Path) -> Result<(), FetchError> {
        if let Some(kind) = self.failed {
            return Err(FetchError::Disk(kind));
        }
        if self.stopped {
            return Err(FetchError::Cancelled);
        }
        let wrong = self.overrun
            || self.misplaced
            || (self.have == artifact.size && hex(&self.hasher.finalize()) != artifact.sha256);
        if wrong {
            let _gone = std::fs::remove_file(part);
            return Err(FetchError::Corrupt);
        }
        if self.have < artifact.size {
            // The connection ended early: the part stays and the next request resumes it.
            return Err(FetchError::Network(HttpError::Malformed));
        }
        Ok(())
    }
}

impl BodySink for Sink {
    fn head(&mut self, status: StatusCode, headers: &HeaderMap) -> Flow {
        match status.as_u16() {
            301 | 302 | 303 | 307 | 308 => {
                self.redirect = headers.header("location").map(str::to_owned);
                Flow::Stop
            }
            200 => {
                // The server sent the whole file whatever was asked: begin again.
                if self.have > 0 && self.start_over().is_err() {
                    self.failed = Some(std::io::ErrorKind::Other);
                    return Flow::Stop;
                }
                self.writing = true;
                Flow::Continue
            }
            206 => {
                let wanted = format!("bytes {}-", self.have);
                self.misplaced = !headers
                    .header("content-range")
                    .is_some_and(|range| range.starts_with(&wanted));
                self.writing = !self.misplaced;
                if self.writing {
                    Flow::Continue
                } else {
                    Flow::Stop
                }
            }
            _ => Flow::Stop,
        }
    }

    fn chunk(&mut self, bytes: &[u8]) -> Flow {
        if !self.writing {
            return Flow::Stop;
        }
        if self.cancel.is_cancelled() {
            self.stopped = true;
            return Flow::Stop;
        }
        if let Err(error) = self.file.write_all(bytes) {
            self.failed = Some(error.kind());
            return Flow::Stop;
        }
        self.hasher.update(bytes);
        self.have += bytes.len() as u64;
        self.meter.add(bytes.len() as u64);
        if self.have > self.expect {
            self.overrun = true;
            return Flow::Stop;
        }
        Flow::Continue
    }
}
