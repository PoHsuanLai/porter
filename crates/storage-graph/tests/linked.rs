//! The replica against a fake Graph whose upload sessions and redirected downloads are on a
//! SECOND loopback origin, as Graph's are on `*.up.1drv.com` and `*.files.1drv.com`: the home
//! origin takes the bearer (the way accountd's `OpenAuthenticated` relay adds it), the linked one
//! takes none and refuses a request that carries any (as OneDrive does). The replica reaches both
//! through `Routed`, and an origin the declaration does not name is never dialled.

mod common;

use common::{TOKEN, TcpDial, client};
use porter_core::{Bytes, Origin, WebUrl};
use porter_fake_servers::graph::{CHUNK_UNIT, Knobs};
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_http::{Http, HttpError, HttpRequest, HttpResponse};
use porter_sync::{
    BaseVersion, Blob, ByteRange, Change, Cursor, ItemPath, PutItem, PutRefused, PutTarget,
    Replica as _, ReplicaError,
};
use std::future::Future;
use storage_graph::{Clock, GraphReplica, Routed, StreamHttp, StreamLimits, Uploads};

/// A client for a linked origin: a dial that adds nothing, or no dial at all for an origin the
/// declaration does not name (accountd's `OpenLinked` refuses it).
struct Linked(Option<StreamHttp<TcpDial>>);

impl Http for Linked {
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        let client = self.0.as_ref();
        async move {
            match client {
                Some(client) => client.send(request).await,
                None => Err(HttpError::Unreachable),
            }
        }
    }
}

type Make = Box<dyn Fn(&Origin) -> Linked + Send + Sync>;

struct Rig {
    graph: Running<GraphHandle>,
    replica: GraphReplica<Routed<common::Client, Linked, Make>>,
}

/// A replica over a linked fake; `declared` says whether the links' origin is one the provider
/// file names.
async fn rig(declared: bool) -> Rig {
    let graph = FakeGraph::start_linked(TOKEN).await.expect("fake graph");
    let port_of = |url: &str| -> u16 {
        url.rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("port")
    };
    let home_port = port_of(graph.base_url());
    let link_port = port_of(graph.link_url().expect("a linked fake"));
    let base = WebUrl::parse(graph.base_url()).expect("url");
    let make: Make = Box::new(
        move |origin: &Origin| match declared && origin.port == link_port {
            true => Linked(Some(StreamHttp::new(
                TcpDial(origin.port),
                StreamLimits::default(),
            ))),
            false => Linked(None),
        },
    );
    let replica = GraphReplica::new(
        Routed::new(client(home_port), &base, make),
        &base,
        "",
        Clock::fixed(common::NOW),
    )
    .with_uploads(Uploads::new(10, CHUNK_UNIT));
    Rig { graph, replica }
}

fn item(path: &str, bytes: Vec<u8>) -> PutItem {
    PutItem {
        target: PutTarget::New(ItemPath(path.into())),
        content: Blob(bytes),
        hash: None,
    }
}

fn big() -> Vec<u8> {
    (0..CHUNK_UNIT * 5 / 2).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn a_large_file_goes_up_and_comes_down_through_the_second_origin() {
    let Rig { graph, replica } = rig(true).await;
    graph.set_knobs(Knobs {
        redirect_downloads: true,
        ..Knobs::default()
    });
    let content = big();
    let (id, _) = replica
        .put(item("dir/big.bin", content.clone()), BaseVersion::Absent)
        .await
        .expect("a session on the second origin");
    assert_eq!(graph.file("dir/big.bin"), Some(content.clone()));

    // Only the session's creation touched the drive's own origin; the three chunks went to the
    // links' origin.
    let home: Vec<(String, String)> = graph
        .hits()
        .into_iter()
        .filter(|h| h.method == "POST")
        .map(|h| (h.method, h.target))
        .collect();
    assert_eq!(home.len(), 1, "{home:?}");
    assert!(
        graph
            .hits()
            .iter()
            .all(|h| !h.target.starts_with("/upload/")),
        "no chunk reached the drive's origin"
    );
    let chunks: Vec<(String, u16)> = graph
        .link_hits()
        .into_iter()
        .map(|h| (h.method, h.status))
        .collect();
    let chunks: Vec<(&str, u16)> = chunks.iter().map(|(m, s)| (m.as_str(), *s)).collect();
    assert_eq!(chunks, [("PUT", 202), ("PUT", 202), ("PUT", 201)]);

    // The download redirects to the links' origin and a range goes with it.
    let span = ByteRange::Span {
        start: Bytes(CHUNK_UNIT as u64 - 2),
        len: Bytes(4),
    };
    assert_eq!(
        replica.fetch(&id, span).await,
        Ok(Blob(content[CHUNK_UNIT - 2..CHUNK_UNIT + 2].to_vec()))
    );
    let page = replica.changes(Cursor::Start).await.expect("listing");
    assert!(matches!(page.changes.first(), Some(Change::Upsert(_))));
    let downloads: Vec<u16> = graph
        .link_hits()
        .into_iter()
        .filter(|h| h.method == "GET")
        .map(|h| h.status)
        .collect();
    assert_eq!(downloads, [206]);
}

#[tokio::test]
async fn no_bearer_and_no_credential_of_any_kind_is_sent_to_the_linked_origin() {
    let Rig { graph, replica } = rig(true).await;
    graph.set_knobs(Knobs {
        redirect_downloads: true,
        ..Knobs::default()
    });
    let (id, _) = replica
        .put(item("a/big.bin", big()), BaseVersion::Absent)
        .await
        .expect("put");
    replica.fetch(&id, ByteRange::Whole).await.expect("fetch");
    let seen = graph.link_hits();
    assert!(seen.len() >= 4, "{seen:?}");
    assert!(
        seen.iter().all(|h| h.authorization.is_none()),
        "the linked origin saw an Authorization: {seen:?}"
    );
    assert!(
        seen.iter().all(|h| h.status != 401),
        "the linked origin refused something: {seen:?}"
    );
    // The home origin did get the bearer (so the test would notice one missing).
    assert!(
        graph
            .hits()
            .iter()
            .any(|h| h.authorization.as_deref() == Some(&format!("Bearer {TOKEN}")))
    );
}

#[tokio::test]
async fn an_origin_the_declaration_does_not_name_is_never_reached() {
    let Rig { graph, replica } = rig(false).await;
    graph.set_knobs(Knobs {
        redirect_downloads: true,
        ..Knobs::default()
    });
    let refused = replica
        .put(item("big.bin", big()), BaseVersion::Absent)
        .await;
    assert!(
        matches!(refused, Err(PutRefused::Transient(_))),
        "{refused:?}"
    );
    assert_eq!(graph.file("big.bin"), None);
    assert!(
        graph.link_hits().is_empty(),
        "nothing was sent to the links' origin"
    );

    graph.put_file("small.bin", b"0123456789");
    let page = replica.changes(Cursor::Start).await.expect("listing");
    let Some(Change::Upsert(found)) = page.changes.first() else {
        panic!("{page:?}")
    };
    let fetched = replica.fetch(&found.id, ByteRange::Whole).await;
    assert!(
        matches!(fetched, Err(ReplicaError::Transient(_))),
        "{fetched:?}"
    );
    assert!(graph.link_hits().is_empty());
}
