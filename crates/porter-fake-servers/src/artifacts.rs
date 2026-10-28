//! A fake file host for downloads: `https` on loopback with the scratch CA (so a client must
//! trust it, as it would any host), files served by path with `Range` support, redirects, and a
//! record of every request. A test puts the bytes it wants served and knows their hash.

use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener, port_of};
use crate::seen::{Seen, lock};
use crate::tls;
use std::collections::BTreeMap;
use std::io;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
struct Shared {
    port: u16,
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    redirects: Arc<Mutex<BTreeMap<String, String>>>,
    hits: Seen<Hit>,
}

/// The test's side of the file host.
#[derive(Debug, Clone)]
pub struct ArtifactsHandle {
    shared: Shared,
}

impl ArtifactsHandle {
    /// The address of `path` (which starts with `/`) on this host.
    pub fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.shared.port)
    }

    /// Serves `body` at `path`.
    pub fn put(&self, path: &str, body: impl Into<Vec<u8>>) {
        lock(&self.shared.files).insert(path.to_owned(), body.into());
    }

    /// Answers `from` with a redirect to `to`, both paths on this host.
    pub fn redirect(&self, from: &str, to: &str) {
        let target = self.url(to);
        lock(&self.shared.redirects).insert(from.to_owned(), target);
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.shared.hits.all()
    }
}

/// A file host serving on a task; dropping it stops the server.
#[derive(Debug)]
pub struct RunningArtifacts {
    handle: ArtifactsHandle,
    task: JoinHandle<()>,
}

impl Deref for RunningArtifacts {
    type Target = ArtifactsHandle;

    fn deref(&self) -> &ArtifactsHandle {
        &self.handle
    }
}

impl Drop for RunningArtifacts {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Binds a file host on loopback and serves it on a task.
///
/// # Errors
/// When the port cannot be bound.
pub async fn start_artifacts() -> io::Result<RunningArtifacts> {
    let listener = Listener::bind(&Bind::Port(0), "artifacts").await?;
    let shared = Shared {
        port: port_of(listener.address()),
        files: Arc::default(),
        redirects: Arc::default(),
        hits: Seen::default(),
    };
    let handle = ArtifactsHandle {
        shared: shared.clone(),
    };
    let task = tokio::spawn(serve(
        listener,
        Some(tls::acceptor()),
        Arc::new(move |request: Request| {
            let response = answer(&shared, &request);
            shared.hits.push(Hit::of(&request, &response));
            response
        }),
    ));
    Ok(RunningArtifacts { handle, task })
}

fn answer(shared: &Shared, request: &Request) -> Response {
    if request.method != "GET" {
        return Response::new(405);
    }
    if let Some(target) = lock(&shared.redirects).get(request.path()) {
        return Response::redirect(target);
    }
    let files = lock(&shared.files);
    let Some(body) = files.get(request.path()) else {
        return Response::new(404);
    };
    let from = request
        .header("range")
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.strip_suffix('-'))
        .and_then(|start| start.parse::<usize>().ok());
    match from {
        None => Response::new(200).typed("application/octet-stream", body.clone()),
        Some(start) if start < body.len() => Response::new(206)
            .with_header(
                "Content-Range",
                &format!("bytes {start}-{}/{}", body.len() - 1, body.len()),
            )
            .typed("application/octet-stream", body[start..].to_vec()),
        Some(_) => Response::new(416),
    }
}
