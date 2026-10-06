//! The rig: a fake DAV server, and machines that sync a library against it through
//! `StreamHttp` over a TCP dial with the credential added the way accountd's relay would.

use super::super::{
    DeviceId, ManualMillis, PhotoLibrary, PhotoMetadata, PhotoOriginals, SystemMillis,
};
use crate::clock::SystemClock;
use crate::engine::{Engine, Report};
use crate::journal::Journal;
use crate::testing::scratch;
use porter_core::stream::ByteStream;
use porter_core::{UnixSeconds, WebUrl};
use porter_fake_servers::{DavHandle, FakeDav, Running};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse};
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use storage_webdav::{Clock, Dial, StreamHttp, StreamLimits, WebDavReplica};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const USER: &str = "ada";
const PASSWORD: &str = "app-password";

#[derive(Debug)]
pub struct Tcp(TcpStream);

impl ByteStream for Tcp {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).await
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.0.shutdown().await
    }
}

#[derive(Debug)]
pub struct TcpDial(u16);

impl Dial for TcpDial {
    type Stream = Tcp;

    async fn dial(&self) -> Result<Tcp, HttpError> {
        TcpStream::connect(("127.0.0.1", self.0))
            .await
            .map(Tcp)
            .map_err(|_| HttpError::Unreachable)
    }
}

/// Adds HTTP Basic credentials to every request, as the relay does on the far side.
#[derive(Debug)]
pub struct Basic<H>(H);

impl<H: Http> Http for Basic<H> {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        use base64::Engine;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASSWORD}"));
        request
            .headers
            .push(Header::new("Authorization", format!("Basic {token}")));
        self.0.send(request)
    }
}

pub type Dav = WebDavReplica<Basic<StreamHttp<TcpDial>>>;

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
            Basic(StreamHttp::new(TcpDial(self.port), StreamLimits::default())),
            &url,
            Clock::fixed(UnixSeconds(1_000)),
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
    pub dir: PathBuf,
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
            dir.join("data/photos"),
            DeviceId::parse(name).expect("device"),
            millis.clone(),
        )
        .expect("library");
        let journal =
            |slug: &str| Journal::open(&dir.join(format!("state/{slug}.sqlite"))).expect("journal");
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
        let camera = self.dir.join("camera");
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
        let camera = self.dir.join("camera");
        std::fs::create_dir_all(&camera).expect("camera");
        let path = camera.join(name);
        std::fs::write(&path, bytes).expect("file");
        path
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The wall clock the real library uses, for a test of the default.
#[allow(dead_code)]
pub fn system_millis() -> SystemMillis {
    SystemMillis
}

impl Synced {
    pub fn is_quiet(&self) -> bool {
        [&self.originals, &self.metadata].iter().all(|r| {
            r.fetched + r.uploaded + r.removed + r.discarded + r.refused + r.conflicts.len() == 0
        })
    }
}
