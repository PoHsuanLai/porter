//! Graph's documented answers (delta pages, a resync, quota, an upload session) through the
//! replica, with a scripted HTTP seam in place of a server: no network, no socket. The fixtures
//! are written from Microsoft's documentation, not recorded from a live account.

use porter_core::{Bytes, UnixSeconds, WebUrl};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse, Status};
use porter_sync::{
    Anchor, BaseVersion, Blob, Change, ContentHash, Cursor, ItemPath, More, PutItem, PutTarget,
    Quota, RemoteId, RemoteItem, RemoteVersion, Replica, ReplicaError, RetryAfter, Tombstone,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use storage_graph::{CHUNK_UNIT, Clock, DELETED, GraphReplica, Uploads};

const ORIGIN: &str = "https://graph.microsoft.com";
const DRIVE: &str = "https://graph.microsoft.com/v1.0/me/drive";
const APPROOT: &str = "https://graph.microsoft.com/v1.0/me/drive/special/approot";
const NOW: UnixSeconds = UnixSeconds(1_000);

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn answer(status: u16, body: impl Into<Vec<u8>>) -> HttpResponse {
    HttpResponse {
        status: Status(status),
        headers: vec![Header::new("Content-Type", "application/json")],
        body: body.into(),
    }
}

/// Answers by `METHOD url`; remembers what it was sent.
#[derive(Debug, Clone, Default)]
struct Script {
    answers: Arc<Mutex<BTreeMap<String, HttpResponse>>>,
    sent: Arc<Mutex<Vec<HttpRequest>>>,
}

impl Script {
    fn on(self, key: &str, response: HttpResponse) -> Self {
        self.answers
            .lock()
            .expect("lock")
            .insert(key.to_owned(), response);
        self
    }

    fn sent(&self) -> Vec<HttpRequest> {
        self.sent.lock().expect("lock").clone()
    }

    fn urls(&self) -> Vec<String> {
        self.sent()
            .iter()
            .map(|r| format!("{} {}", r.method.token(), r.url.as_str()))
            .collect()
    }
}

impl Http for Script {
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        let key = format!("{} {}", request.method.token(), request.url.as_str());
        self.sent.lock().expect("lock").push(request);
        let found = self.answers.lock().expect("lock").get(&key).cloned();
        async move { found.ok_or(HttpError::Unreachable) }
    }
}

fn replica(script: Script) -> GraphReplica<Script> {
    GraphReplica::new(
        script,
        &WebUrl::parse(ORIGIN).expect("url"),
        "",
        Clock::fixed(NOW),
    )
}

fn with_approot(script: Script) -> Script {
    script.on(
        &format!("GET {APPROOT}"),
        answer(200, fixture("approot.json")),
    )
}

#[tokio::test]
async fn a_delta_in_two_pages_is_read_as_the_documented_answers_have_them() {
    let script = with_approot(Script::default())
        .on(
            &format!("GET {APPROOT}/delta"),
            answer(200, fixture("delta_page1.json")),
        )
        .on(
            &format!("GET {APPROOT}/delta?$skiptoken=MTIzNDU2"),
            answer(200, fixture("delta_page2.json")),
        );
    let replica = replica(script.clone());

    let first = replica.changes(Cursor::Start).await.expect("page 1");
    // The file names its folder, which comes after it in the page and is a folder this page
    // names; the folder, the OneNote package and the app folder itself are not changes.
    assert_eq!(
        first.changes,
        vec![Change::Upsert(RemoteItem {
            id: RemoteId("01IMG0000000000000000000000001".into()),
            version: RemoteVersion("\"{01IMG1},3\"".into()),
            path: ItemPath("2026/IMG_0001.HEIC".into()),
            size: Bytes(2048),
            hash: Some(ContentHash(
                "000102030405060708090a0b0c0d0e0f10111213".into()
            )),
        })]
    );
    assert_eq!(first.more, More::More);
    assert_eq!(
        first.next,
        Anchor(format!("{APPROOT}/delta?$skiptoken=MTIzNDU2"))
    );

    let second = replica
        .changes(Cursor::At(first.next))
        .await
        .expect("page 2");
    assert_eq!(
        second.changes,
        vec![
            Change::Tombstone(Tombstone {
                id: RemoteId("01OLD0000000000000000000000001".into()),
                version: RemoteVersion(DELETED.into()),
                deleted_at: NOW,
            }),
            Change::Upsert(RemoteItem {
                id: RemoteId("01TOP0000000000000000000000001".into()),
                version: RemoteVersion("\"{01TOP},1\"".into()),
                path: ItemPath("cover.jpg".into()),
                size: Bytes(12),
                hash: None,
            }),
        ],
        "a shortcut (remoteItem) is not synced"
    );
    assert_eq!(second.more, More::Done);
    assert_eq!(
        second.next,
        Anchor(format!("{APPROOT}/delta?token=MjAyNi0wOS0zMFQwODowMjowMFo"))
    );
    // Nothing the replica sent carries a credential: the relay adds it.
    assert!(
        script
            .sent()
            .iter()
            .all(|r| r.headers.iter().all(|h| h.name.as_str() != "authorization"))
    );
}

#[tokio::test]
async fn a_file_whose_folder_the_feed_never_named_asks_for_the_chain_of_folders() {
    let page = format!(
        r#"{{"value":[{}],"@odata.deltaLink":"{APPROOT}/delta?token=T1"}}"#,
        fixture("item_moved_in.json")
    );
    let year = r#"{"id":"01YEAR0000000000000000000000026","name":"2026","eTag":"\"{01YEAR},1\"","parentReference":{"id":"01APPROOT0000000000000000000000"},"folder":{}}"#;
    let script = with_approot(Script::default())
        .on(&format!("GET {APPROOT}/delta?token=T0"), answer(200, page))
        .on(
            &format!("GET {DRIVE}/items/01SUB00000000000000000000000001"),
            answer(200, fixture("folder_sub.json")),
        )
        .on(
            &format!("GET {DRIVE}/items/01YEAR0000000000000000000000026"),
            answer(200, year),
        );
    let replica = replica(script.clone());
    let page = replica
        .changes(Cursor::At(Anchor(format!("{APPROOT}/delta?token=T0"))))
        .await
        .expect("page");
    let paths: Vec<&ItemPath> = page
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) => Some(&item.path),
            Change::Tombstone(_) => None,
        })
        .collect();
    assert_eq!(paths, vec![&ItemPath("2026/sub/deep.jpg".into())]);
    assert_eq!(
        script.urls(),
        vec![
            format!("GET {APPROOT}/delta?token=T0"),
            // The dataset's folder is asked for when the first path needs it.
            format!("GET {APPROOT}"),
            format!("GET {DRIVE}/items/01SUB00000000000000000000000001"),
            format!("GET {DRIVE}/items/01YEAR0000000000000000000000026"),
        ]
    );
}

#[tokio::test]
async fn a_resync_is_an_expired_anchor_and_a_link_from_elsewhere_is_one_without_a_request() {
    let script = with_approot(Script::default()).on(
        &format!("GET {APPROOT}/delta?token=OLD"),
        answer(410, fixture("resync_required.json")),
    );
    let replica = replica(script.clone());
    let expired = replica
        .changes(Cursor::At(Anchor(format!("{APPROOT}/delta?token=OLD"))))
        .await;
    assert_eq!(expired.unwrap_err(), ReplicaError::AnchorExpired);

    let before = script.sent().len();
    for link in [
        "https://evil.example/v1.0/me/drive/special/approot/delta?token=x",
        "https://graph.microsoft.com.evil.example/v1.0/x",
        "not a link",
    ] {
        let foreign = replica.changes(Cursor::At(Anchor(link.into()))).await;
        assert_eq!(foreign.unwrap_err(), ReplicaError::AnchorExpired, "{link}");
    }
    assert_eq!(script.sent().len(), before, "nothing was sent to them");
}

#[tokio::test]
async fn a_throttled_read_says_when_to_come_back_and_a_refused_token_is_unauthorized() {
    let throttled = HttpResponse {
        status: Status(429),
        headers: vec![Header::new("Retry-After", "17")],
        body: br#"{"error":{"code":"activityLimitReached"}}"#.to_vec(),
    };
    let script = Script::default()
        .on(&format!("GET {DRIVE}"), throttled)
        .on(&format!("GET {APPROOT}"), answer(401, "{}"));
    let replica = replica(script);
    assert_eq!(
        replica.quota().await,
        Err(ReplicaError::Transient(RetryAfter(17)))
    );
    assert_eq!(
        replica.changes(Cursor::Start).await.unwrap_err(),
        ReplicaError::Unauthorized
    );
}

#[tokio::test]
async fn quota_is_used_and_total_and_a_total_of_zero_is_no_limit() {
    for (file, want) in [
        (
            "drive_business.json",
            Quota {
                used: Bytes(1024),
                total: Some(Bytes(1_099_511_628_800)),
            },
        ),
        (
            "drive_unlimited.json",
            Quota {
                used: Bytes(5120),
                total: None,
            },
        ),
    ] {
        let script = Script::default().on(&format!("GET {DRIVE}"), answer(200, fixture(file)));
        assert_eq!(replica(script).quota().await, Ok(want), "{file}");
    }
}

#[tokio::test]
async fn a_session_upload_goes_to_the_url_it_names_in_ranges_and_conditions_the_session() {
    let done = r#"{"id":"01NEW","name":"big.bin","eTag":"\"{01NEW},1\"","parentReference":{"id":"01APPROOT0000000000000000000000"},"file":{}}"#;
    let upload = "https://sn3302.up.1drv.com/up/fe6987415ace7X4e1eF866337";
    let script = Script::default()
        .on(
            &format!("POST {APPROOT}:/big.bin:/createUploadSession"),
            answer(200, fixture("session.json")),
        )
        .on(&format!("PUT {upload}"), answer(201, done));
    let replica = replica(script.clone()).with_uploads(Uploads::new(0, CHUNK_UNIT));
    let put = replica
        .put(
            PutItem {
                target: PutTarget::New(ItemPath("big.bin".into())),
                content: Blob(b"hello".to_vec()),
                hash: None,
            },
            BaseVersion::Absent,
        )
        .await;
    assert_eq!(
        put,
        Ok((
            RemoteId("01NEW".into()),
            RemoteVersion("\"{01NEW},1\"".into())
        ))
    );
    let sent = script.sent();
    let create = &sent[0];
    assert_eq!(
        String::from_utf8_lossy(&create.body),
        r#"{"item":{"@microsoft.graph.conflictBehavior":"fail"}}"#
    );
    let chunk = &sent[1];
    let range = chunk
        .headers
        .iter()
        .find(|h| h.name.as_str() == "content-range")
        .map(|h| h.value.0.as_str());
    assert_eq!(range, Some("bytes 0-4/5"));
    assert_eq!(chunk.body, b"hello");
}
