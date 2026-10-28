//! Recorded multistatus fixtures (the shapes Nextcloud and Sabre-based servers answer) through
//! the replica, with a scripted HTTP seam in place of a server: no network, no socket.

use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use porter_ids::{Bytes, UnixSeconds, WebUrl};
use porter_sync::Clock;
use porter_sync::{
    Change, Cursor, Quota, RemoteId, RemoteVersion, Replica, ReplicaError, RetryAfter,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use storage_webdav::WebDavReplica;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn answer(status: u16, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/xml")
        .body(body)
        .expect("response")
}

type Answers = Arc<Mutex<BTreeMap<String, Vec<Response<Vec<u8>>>>>>;

/// Answers by `METHOD target`, in the order given for a key and then the last one again;
/// remembers what it was sent.
#[derive(Debug, Clone, Default)]
struct Script {
    answers: Answers,
    sent: Arc<Mutex<Vec<Request<Vec<u8>>>>>,
}

impl Script {
    fn on(self, key: &str, response: Response<Vec<u8>>) -> Self {
        self.answers
            .lock()
            .expect("lock")
            .entry(key.to_owned())
            .or_default()
            .push(response);
        self
    }

    fn sent(&self) -> Vec<Request<Vec<u8>>> {
        self.sent.lock().expect("lock").clone()
    }
}

impl Http for Script {
    fn send(
        &self,
        request: Request<Vec<u8>>,
    ) -> impl Future<Output = Result<Response<Vec<u8>>, HttpError>> + Send {
        let key = format!("{} {}", request.method(), request.uri().path());
        self.sent.lock().expect("lock").push(request);
        let found = self
            .answers
            .lock()
            .expect("lock")
            .get_mut(&key)
            .and_then(|queue| match queue.len() {
                0 | 1 => queue.first().cloned(),
                _ => Some(queue.remove(0)),
            });
        async move { found.ok_or(HttpError::Unreachable) }
    }
}

fn replica(script: Script) -> WebDavReplica<Script> {
    let folder =
        WebUrl::parse("https://cloud.example.org/remote.php/dav/files/alice/Photos/").expect("url");
    WebDavReplica::new(script, &folder, Clock::fixed(UnixSeconds(5)), "n1")
}

const FOLDER: &str = "/remote.php/dav/files/alice/Photos/";

#[tokio::test]
async fn a_nextcloud_files_listing_becomes_items_at_their_etags() {
    let script = Script::default()
        .on(
            "REPORT /remote.php/dav/files/alice/Photos/",
            answer(501, b"Not Implemented".to_vec()),
        )
        .on(
            &format!("PROPFIND {FOLDER}"),
            answer(207, fixture("nextcloud-files-photos.xml")),
        )
        .on(
            &format!("PROPFIND {FOLDER}2026/"),
            answer(207, fixture("nextcloud-files-2026.xml")),
        );
    let page = replica(script)
        .changes(Cursor::Start)
        .await
        .expect("listing");
    let said: Vec<(String, String, String, u64)> = page
        .changes
        .iter()
        .map(|c| match c {
            Change::Upsert(item) => (
                item.id.0.clone(),
                item.path.0.clone(),
                item.version.0.clone(),
                item.size.0,
            ),
            Change::Tombstone(t) => panic!("{t:?}"),
        })
        .collect();
    assert_eq!(
        said,
        [
            (
                format!("{FOLDER}2026/IMG%201234.HEIC"),
                "2026/IMG 1234.HEIC".to_owned(),
                "\"5f3a9c0d1e2b4a6c8d7e9f0a1b2c3d4e\"".to_owned(),
                2_345_678
            ),
            (
                format!("{FOLDER}cover.jpg"),
                "cover.jpg".to_owned(),
                "\"a1b2c3d4e5f60718293a4b5c6d7e8f90\"".to_owned(),
                48_211
            ),
        ]
    );
}

#[tokio::test]
async fn a_sabre_sync_report_gives_changes_removals_and_the_next_token() {
    let script = Script::default().on(
        &format!("REPORT {FOLDER}"),
        answer(207, fixture("sabre-sync-report.xml")),
    );
    let replica = replica(script.clone());
    let page = replica
        .changes(Cursor::At(porter_sync::Anchor(
            "sync:http://sabre.test/sync/41".into(),
        )))
        .await;
    // Nothing was listed in this process, so a removal cannot be taken apart: the anchor is
    // expired and the engine lists again.
    assert_eq!(page, Err(ReplicaError::AnchorExpired));
    let sent = script.sent();
    let body = String::from_utf8(sent[0].body().clone()).expect("utf8");
    assert!(body.contains("http://sabre.test/sync/41"), "{body}");
    assert!(body.contains("infinite"));
}

/// A report a server cut short (RFC 6578 section 3.6): one file, the 507 for the folder, and
/// the token that continues it.
fn truncated_report() -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
 <d:response><d:href>{FOLDER}a.jpg</d:href>
  <d:propstat><d:prop><d:getetag>"a1"</d:getetag><d:getcontentlength>3</d:getcontentlength><d:resourcetype/></d:prop>
  <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>{FOLDER}</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>
 <d:sync-token>http://sabre.test/sync/7</d:sync-token>
</d:multistatus>"#
    )
    .into_bytes()
}

#[tokio::test]
async fn a_truncated_report_continues_from_its_token_instead_of_expiring() {
    let script = Script::default()
        .on(&format!("REPORT {FOLDER}"), answer(207, truncated_report()))
        .on(
            &format!("REPORT {FOLDER}"),
            answer(207, fixture("sabre-sync-report.xml")),
        );
    let replica = replica(script.clone());
    let first = replica.changes(Cursor::Start).await.expect("first page");
    let ids: Vec<&str> = first
        .changes
        .iter()
        .map(|c| match c {
            Change::Upsert(item) => item.id.0.as_str(),
            Change::Tombstone(t) => panic!("{t:?}"),
        })
        .collect();
    assert_eq!(ids, [format!("{FOLDER}a.jpg")]);
    // Not the end: the rest is asked for at once, from the token the server gave.
    assert_eq!(first.more, porter_sync::More::More);
    assert_eq!(
        first.next,
        porter_sync::Anchor("sync:http://sabre.test/sync/7".into())
    );
    let rest = replica
        .changes(Cursor::At(first.next))
        .await
        .expect("the rest");
    assert_eq!(rest.more, porter_sync::More::Done);
    assert_eq!(
        rest.next,
        porter_sync::Anchor("sync:http://sabre.test/sync/42".into())
    );
    let sent = script.sent();
    assert_eq!(sent.len(), 2);
    let body = String::from_utf8(sent[1].body().clone()).expect("utf8");
    assert!(body.contains("http://sabre.test/sync/7"), "{body}");
}

#[tokio::test]
async fn a_valid_sync_token_precondition_is_an_expired_anchor() {
    let script = Script::default().on(
        &format!("REPORT {FOLDER}"),
        answer(403, fixture("sabre-sync-expired.xml")),
    );
    let got = replica(script)
        .changes(Cursor::At(porter_sync::Anchor("sync:old".into())))
        .await;
    assert_eq!(got, Err(ReplicaError::AnchorExpired));
}

#[tokio::test]
async fn nextcloud_quota_is_used_plus_available_and_unlimited_is_no_total() {
    let limited = Script::default().on(
        &format!("PROPFIND {FOLDER}"),
        answer(207, fixture("nextcloud-files-quota.xml")),
    );
    assert_eq!(
        replica(limited).quota().await,
        Ok(Quota {
            used: Bytes(2_393_889),
            total: Some(Bytes(2_393_889 + 10_737_418_240))
        })
    );
    let unlimited = Script::default().on(
        &format!("PROPFIND {FOLDER}"),
        answer(207, fixture("nextcloud-files-quota-unlimited.xml")),
    );
    assert_eq!(
        replica(unlimited).quota().await,
        Ok(Quota {
            used: Bytes(500),
            total: None
        })
    );
}

#[tokio::test]
async fn fetch_and_write_statuses_map_to_the_contract() {
    let id = RemoteId(format!("{FOLDER}a.jpg"));
    let key = |m: &str| format!("{m} {FOLDER}a.jpg");
    let ignoring_range = Script::default().on(&key("GET"), answer(200, b"0123456789".to_vec()));
    let span = porter_sync::ByteRange::Span {
        start: Bytes(2),
        len: Bytes(3),
    };
    assert_eq!(
        replica(ignoring_range).fetch(&id, span).await,
        Ok(porter_sync::Blob(b"234".to_vec()))
    );
    let past_end = Script::default().on(&key("GET"), answer(416, vec![]));
    assert_eq!(
        replica(past_end).fetch(&id, span).await,
        Ok(porter_sync::Blob(vec![]))
    );
    let busy = Response::builder()
        .status(503)
        .header("Retry-After", "17")
        .body(vec![])
        .expect("response");
    let script = Script::default()
        .on(&key("GET"), busy.clone())
        .on(&key("PUT"), busy);
    let r = replica(script);
    assert_eq!(
        r.fetch(&id, porter_sync::ByteRange::Whole).await,
        Err(ReplicaError::Transient(RetryAfter(17)))
    );
    let put = porter_sync::PutItem {
        target: porter_sync::PutTarget::Existing(id),
        content: porter_sync::Blob(b"x".to_vec()),
        hash: None,
    };
    assert_eq!(
        r.put(
            put,
            porter_sync::BaseVersion::At(RemoteVersion("\"e\"".into()))
        )
        .await,
        Err(porter_sync::PutRefused::Transient(RetryAfter(17)))
    );
}
