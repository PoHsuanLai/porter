//! A second origin for the bytes of picked photos, as Google serves them from
//! `lh3.googleusercontent.com`: a different host from the Picker API, and one that wants the
//! account's bearer on every download. Additive to the fake Google: `GoogleHandle::serve_media`
//! starts it and points the picked items' `baseUrl`s at it.

use super::{Shared, denied};
use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen, lock};
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use std::future::Future;
use std::io;
use std::sync::Arc;

/// The test's side of the media origin.
#[derive(Debug, Clone)]
pub struct MediaOrigin {
    base: String,
    hits: Seen<Hit>,
}

impl MediaOrigin {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.hits.all()
    }
}

struct MediaServer {
    listener: Listener,
    shared: Shared,
    hits: Seen<Hit>,
}

fn answer(shared: &Shared, request: &Request) -> Response {
    let Some(id) = request.path().strip_prefix("/dl/") else {
        return Response::new(404);
    };
    let state = lock(&shared.state);
    let presented = request.bearer();
    let good = presented.is_some_and(|token| {
        state.fixed.iter().any(|f| f == token)
            || shared
                .issuer
                .as_ref()
                .is_some_and(|issuer| issuer.access_is_live(token))
    });
    if !good {
        return denied(401, "UNAUTHENTICATED");
    }
    // `=d` (bytes) or `=dv` (video bytes) is the suffix Google's baseUrls take.
    let id = id.split('=').next().unwrap_or(id);
    match state.photos.picked(id) {
        Some((mime, bytes)) => Response::new(200).typed(&mime, bytes),
        None => denied(404, "NOT_FOUND"),
    }
}

impl FakeServer for MediaServer {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Google
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        spec.clone()
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let (shared, hits) = (self.shared, self.hits);
        serve(
            self.listener,
            None,
            Arc::new(move |request: Request| {
                let response = answer(&shared, &request);
                hits.push(Hit::of(&request, &response));
                response
            }),
        )
    }
}

impl super::GoogleHandle {
    /// Starts a second origin that serves the picked items' bytes at `/dl/<id>` and wants the
    /// account's bearer, and points the picked items' `baseUrl`s at it.
    pub async fn serve_media(&self) -> io::Result<Running<MediaOrigin>> {
        let listener = Listener::bind(&Bind::Loopback, "google-media").await?;
        let base = format!(
            "http://127.0.0.1:{}",
            crate::net::port_of(listener.address())
        );
        lock(&self.shared.state).photos.media_base = Some(base.clone());
        let hits = Seen::default();
        let handle = MediaOrigin {
            base,
            hits: hits.clone(),
        };
        let server = MediaServer {
            listener,
            shared: self.shared.clone(),
            hits,
        };
        Ok(Running::spawn(server, handle))
    }
}
