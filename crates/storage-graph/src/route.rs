//! [`Routed`]: the one `Http` a Graph replica is handed, which keeps the credentialed host and
//! the linked hosts apart. Graph answers an upload session with an `uploadUrl`, and a content
//! request with a redirect to a `downloadUrl`; both are pre-authenticated and on other hosts.
//! A request to the home origin goes to the home `Http` (in syncd, accountd's authenticated
//! relay, which adds the bearer); a request to any other origin goes to an `Http` made for that
//! origin (in syncd, accountd's `OpenLinked` relay, which adds nothing and which accountd
//! refuses for an origin the provider file does not declare). The replica itself never adds an
//! `Authorization` header, so a credential cannot follow a link to another host.

use porter_core::{Origin, WebUrl};
use porter_http::{Http, HttpError, HttpRequest, HttpResponse};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// How many linked origins are kept at once; past it the oldest are forgotten and made again.
const KEPT: usize = 8;

/// An [`Http`] that sends to `home` what is for `home_origin` and to `Linked` clients what is for
/// any other origin.
pub struct Routed<H, L, F> {
    home: H,
    home_origin: Origin,
    make: F,
    linked: Mutex<HashMap<Origin, Arc<L>>>,
}

impl<H, L, F> std::fmt::Debug for Routed<H, L, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Routed")
            .field("home_origin", &self.home_origin)
            .finish_non_exhaustive()
    }
}

impl<H, L, F> Routed<H, L, F>
where
    F: Fn(&Origin) -> L,
{
    /// A router: `home` for `home_origin`, `make(origin)` for each other origin a request names.
    pub fn new(home: H, home_origin: &WebUrl, make: F) -> Self {
        Self {
            home,
            home_origin: home_origin.origin(),
            make,
            linked: Mutex::new(HashMap::new()),
        }
    }

    fn linked(&self, origin: Origin) -> Arc<L> {
        let mut linked = self.linked.lock().unwrap_or_else(PoisonError::into_inner);
        if linked.len() >= KEPT && !linked.contains_key(&origin) {
            linked.clear();
        }
        Arc::clone(
            linked
                .entry(origin.clone())
                .or_insert_with(|| Arc::new((self.make)(&origin))),
        )
    }
}

impl<H, L, F> Http for Routed<H, L, F>
where
    H: Http,
    L: Http,
    F: Fn(&Origin) -> L + Send + Sync,
{
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let origin = request.url.origin();
        if origin == self.home_origin {
            return self.home.send(request).await;
        }
        self.linked(origin).send(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::{Method, Status};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Answers with its own name.
    struct Named(&'static str, Arc<AtomicUsize>);

    impl Http for Named {
        async fn send(&self, _: HttpRequest) -> Result<HttpResponse, HttpError> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(HttpResponse {
                status: Status(200),
                headers: Vec::new(),
                body: self.0.as_bytes().to_vec(),
            })
        }
    }

    fn get(url: &str) -> HttpRequest {
        HttpRequest::new(Method::Get, WebUrl::parse(url).expect("url"))
    }

    #[tokio::test]
    async fn the_home_origin_and_the_others_are_kept_apart_and_a_linked_client_is_made_once() {
        let made = Arc::new(AtomicUsize::new(0));
        let routed = Routed::new(
            Named("home", Arc::default()),
            &WebUrl::parse("https://graph.example/v1.0").expect("url"),
            {
                let made = Arc::clone(&made);
                move |_: &Origin| {
                    made.fetch_add(1, Ordering::SeqCst);
                    Named("linked", Arc::default())
                }
            },
        );
        let body = |url: &str| {
            let routed = &routed;
            let request = get(url);
            async move { routed.send(request).await.expect("sent").body }
        };
        assert_eq!(body("https://graph.example/v1.0/me/drive").await, b"home");
        assert_eq!(body("https://up.example/up/s1").await, b"linked");
        assert_eq!(body("https://up.example/up/s1?x=1").await, b"linked");
        assert_eq!(body("https://down.example/d").await, b"linked");
        // Another port is another origin.
        assert_eq!(body("https://graph.example:8443/x").await, b"linked");
        assert_eq!(made.load(Ordering::SeqCst), 3);
    }
}
