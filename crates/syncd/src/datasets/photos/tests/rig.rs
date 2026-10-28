//! The rig: a fake DAV server, and machines that sync a library against it through
//! `RelayHttp` over a TCP dial with the credential added the way accountd's relay would.

use super::super::{DeviceId, ManualMillis, PhotoLibrary, PhotoMetadata, PhotoOriginals};
use crate::engine::{Engine, Report};
use crate::journal::Journal;
use crate::testing::scratch;
use porter_fake_servers::{DavHandle, FakeDav, Running};
use porter_http::http::header::{AUTHORIZATION, HeaderValue};
use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use porter_http_relay::{Dial, RelayHttp, RelayLimits};
use porter_ids::clock::SystemClock;
use porter_ids::{UnixSeconds, WebUrl};
use porter_sync::Clock;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use storage_webdav::WebDavReplica;
use tokio::net::TcpStream;

const USER: &str = "ada";
const PASSWORD: &str = "app-password";

#[derive(Debug)]
pub struct TcpDial(u16);

impl Dial for TcpDial {
    type Stream = TcpStream;

    async fn dial(&self) -> Result<TcpStream, HttpError> {
        let stream = TcpStream::connect(("127.0.0.1", self.0))
            .await
            .map_err(|_| HttpError::Unreachable)?;
        // A request written in two parts must not wait for the server's delayed
        // acknowledgement: that is 40 ms a request on loopback.
        let _ = stream.set_nodelay(true);
        Ok(stream)
    }
}

/// Adds HTTP Basic credentials to every request, as the relay does on the far side.
#[derive(Debug)]
pub struct Basic<H>(H);

impl<H: Http> Http for Basic<H> {
    fn send(
        &self,
        mut request: Request<Vec<u8>>,
    ) -> impl Future<Output = Result<Response<Vec<u8>>, HttpError>> + Send {
        use base64::Engine;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASSWORD}"));
        let mut value =
            HeaderValue::from_str(&format!("Basic {token}")).expect("base64 is a value");
        value.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, value);
        self.0.send(request)
    }
}

pub type Dav = WebDavReplica<Basic<RelayHttp<TcpDial>>>;

/// The fake server with the two folders of a Photos account made.
pub struct Server {
    pub dav: Running<DavHandle>,
    port: u16,
}

impl Server {
    pub async fn start() -> Self {
        let dav = FakeDav::start(USER, PASSWORD).await.expect("fake dav");
        for folder in ["photos", "photos/originals", "photos/metadata"] {
            dav.mkdir(folder);
        }
        let port = dav
            .base_url()
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("a port");
        Self { dav, port }
    }

    /// A new replica of `folder` (a new process, as far as anchors go).
    pub fn replica(&self, folder: &str) -> Dav {
        let url = WebUrl::parse(&format!(
            "{}/dav/files/photos/{folder}/",
            self.dav.base_url()
        ))
        .expect("url");
        WebDavReplica::new(
            Basic(RelayHttp::new(TcpDial(self.port), RelayLimits::default())),
            &url,
            Clock::fixed(UnixSeconds(1_000)),
            "n1",
        )
    }

    /// How many PUTs the server answered with success under `folder`.
    pub fn puts(&self, folder: &str) -> usize {
        let prefix = format!("/dav/files/photos/{folder}/");
        self.dav
            .hits()
            .iter()
            .filter(|h| h.method == "PUT" && h.target.starts_with(&prefix) && h.status < 300)
            .count()
    }
}

pub struct Synced {
    pub originals: Report,
    pub metadata: Report,
}

/// One machine: its own scratch directory, library, journals and engines.
pub struct Machine {
    pub dir: tempfile::TempDir,
    pub library: PhotoLibrary,
    pub millis: Arc<ManualMillis>,
    pub originals: Engine<Dav, PhotoOriginals, SystemClock>,
    pub metadata: Engine<Dav, PhotoMetadata, SystemClock>,
}

impl Machine {
    /// A machine named `name` whose wall clock reads `ms`.
    pub fn new(server: &Server, name: &str, ms: u64) -> Self {
        let dir = scratch(&format!("photos-{name}"));
        let millis = Arc::new(ManualMillis::at(ms));
        let library = PhotoLibrary::open(
            dir.path().join("data/photos"),
            DeviceId::parse(name).expect("device"),
            millis.clone(),
        )
        .expect("library");
        // Not flushed on each commit: a thousand photos make thousands of them, and these tests
        // prove what the engine does, not what the disk does.
        let journal = |slug: &str| {
            Journal::open_unflushed(&dir.path().join(format!("state/{slug}.sqlite")))
                .expect("journal")
        };
        let originals = Engine::new(
            server.replica("originals"),
            PhotoOriginals::new(library.clone()),
            journal("originals"),
            SystemClock,
        );
        let metadata = Engine::new(
            server.replica("metadata"),
            PhotoMetadata::new(library.clone()),
            journal("metadata"),
            SystemClock,
        );
        Self {
            dir,
            library,
            millis,
            originals,
            metadata,
        }
    }

    /// One cycle of each dataset, as the daemon's two drivers would run them.
    pub async fn sync(&self) -> Synced {
        Synced {
            originals: self.originals.sync_once().await.expect("originals cycle"),
            metadata: self.metadata.sync_once().await.expect("metadata cycle"),
        }
    }

    /// `n` tiny distinct fixture files in this machine's camera folder (never committed:
    /// generated here). `seed` makes a second batch differ from the first.
    pub fn fixtures(&self, seed: &str, n: usize) -> Vec<PathBuf> {
        let camera = self.dir.path().join("camera");
        std::fs::create_dir_all(&camera).expect("camera");
        (0..n)
            .map(|i| {
                let path = camera.join(format!("IMG_{seed}_{i:04}.jpg"));
                std::fs::write(&path, format!("fixture photo {seed} {i:04}\n").repeat(3))
                    .expect("fixture");
                path
            })
            .collect()
    }

    /// One file with chosen bytes.
    pub fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let camera = self.dir.path().join("camera");
        std::fs::create_dir_all(&camera).expect("camera");
        let path = camera.join(name);
        std::fs::write(&path, bytes).expect("file");
        path
    }
}

impl Synced {
    pub fn is_quiet(&self) -> bool {
        [&self.originals, &self.metadata].iter().all(|r| {
            r.fetched + r.uploaded + r.removed + r.discarded + r.refused + r.conflicts.len() == 0
        })
    }
}
