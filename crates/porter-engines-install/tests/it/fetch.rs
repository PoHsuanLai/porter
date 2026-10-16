use crate::support::fetcher;
use porter_engines::{Artifact, file_digest};
use porter_engines_install::{CancelToken, FetchError, Meter};
use porter_fake::Scratch;
use porter_fake_servers::start_artifacts;
use porter_http::HttpError;
use sha2::{Digest, Sha256};

fn artifact(url: String, body: &[u8]) -> Artifact {
    let digest = Sha256::digest(body);
    Artifact {
        url: Box::leak(url.into_boxed_str()),
        sha256: Box::leak(porter_engines::hex(&digest).into_boxed_str()),
        size: body.len() as u64,
    }
}

fn body() -> Vec<u8> {
    (0..200_000u32).map(|n| (n % 251) as u8).collect()
}

#[tokio::test]
async fn a_file_is_downloaded_counted_and_kept_only_when_it_is_the_pinned_one() {
    let dir = Scratch::new("fetch-ok");
    let host = start_artifacts().await.expect("host");
    host.put("/a", body());
    let wanted = artifact(host.url("/a"), &body());
    let (dest, meter) = (dir.path().join("a.tar.gz"), Meter::new(wanted.size));
    fetcher()
        .fetch(&wanted, &dest, &meter, &CancelToken::new())
        .await
        .expect("fetched");
    assert_eq!(std::fs::read(&dest).expect("file"), body());
    assert_eq!(meter.permille(), 1000);
    assert!(!dir.path().join("a.tar.gz.part").exists());
    // Already there and right: not asked for again.
    let before = host.hits().len();
    fetcher()
        .fetch(
            &wanted,
            &dest,
            &Meter::new(wanted.size),
            &CancelToken::new(),
        )
        .await
        .expect("kept");
    assert_eq!(host.hits().len(), before);
}

#[tokio::test]
async fn a_redirect_is_followed_and_a_range_resumes_a_part() {
    let dir = Scratch::new("fetch-resume");
    let host = start_artifacts().await.expect("host");
    host.put("/real", body());
    host.redirect("/go", "/real");
    let wanted = artifact(host.url("/go"), &body());
    let dest = dir.path().join("f");
    std::fs::write(dir.path().join("f.part"), &body()[..70_000]).expect("part");
    let meter = Meter::new(wanted.size);
    fetcher()
        .fetch(&wanted, &dest, &meter, &CancelToken::new())
        .await
        .expect("fetched");
    assert_eq!(file_digest(&dest).expect("digest").1, wanted.sha256);
    let hits = host.hits();
    assert_eq!(hits.len(), 2);
    assert_eq!((hits[0].target.as_str(), hits[0].status), ("/go", 302));
    assert_eq!(hits[1].range.as_deref(), Some("bytes=70000-"));
    assert_eq!(hits[1].status, 206);
    assert_eq!(meter.permille(), 1000);
}

#[tokio::test]
async fn what_is_not_the_pinned_file_is_deleted_and_an_error_status_is_refused() {
    let dir = Scratch::new("fetch-wrong");
    let host = start_artifacts().await.expect("host");
    host.put("/a", body());
    let mut wanted = artifact(host.url("/a"), &body());
    wanted.sha256 = Box::leak("0".repeat(64).into_boxed_str());
    let dest = dir.path().join("a");
    let outcome = fetcher()
        .fetch(
            &wanted,
            &dest,
            &Meter::new(wanted.size),
            &CancelToken::new(),
        )
        .await;
    assert_eq!(outcome, Err(FetchError::Corrupt));
    assert!(!dest.exists() && !dir.path().join("a.part").exists());
    let missing = artifact(host.url("/none"), &body());
    let outcome = fetcher()
        .fetch(&missing, &dest, &Meter::new(1), &CancelToken::new())
        .await;
    assert_eq!(outcome, Err(FetchError::Refused(404)));
}

#[tokio::test]
async fn plain_http_an_unreachable_host_and_a_stop_end_the_download_as_what_they_are() {
    let dir = Scratch::new("fetch-ends");
    let dest = dir.path().join("a");
    let plain = artifact("http://127.0.0.1:1/a".to_owned(), b"x");
    let outcome = fetcher()
        .fetch(&plain, &dest, &Meter::new(1), &CancelToken::new())
        .await;
    assert_eq!(outcome, Err(FetchError::BadAddress));
    let nowhere = artifact("https://localhost:1/a".to_owned(), b"x");
    let outcome = fetcher()
        .fetch(&nowhere, &dest, &Meter::new(1), &CancelToken::new())
        .await;
    assert_eq!(outcome, Err(FetchError::Network(HttpError::Unreachable)));
    let host = start_artifacts().await.expect("host");
    host.put("/a", body());
    let wanted = artifact(host.url("/a"), &body());
    let stopped = CancelToken::new();
    stopped.cancel();
    let outcome = fetcher()
        .fetch(&wanted, &dest, &Meter::new(wanted.size), &stopped)
        .await;
    assert_eq!(outcome, Err(FetchError::Cancelled));
    assert!(!dest.exists());
}
