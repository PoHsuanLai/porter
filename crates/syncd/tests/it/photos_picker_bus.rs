//! `org.quire.Photos1.Picker` on syncd's bus name, against the real accountd and the fake
//! Google: the Photos app starts a session, polls it, imports what the person picked and cancels
//! another; nobody else may; a malformed word or an account with no Picker is refused with a
//! typed error; and no token or Google URL crosses the bus.

use crate::common;
use crate::googlerig as google_rig;

use common::{ACCESS_DENIED, INVALID_ARGS, NO_FITTING, error_name};
use google_rig::{
    PICKER_BEARER, Rig, SECRET_ACCESS, SECRET_REFRESH, SEGMENT, client_of, rig, rig_picker_only,
    serve_picker,
};
use porter_dbus::{
    PICKER_ERROR_NO_SUCH_SESSION, PICKER_ERROR_NOT_YET, PICKER_PICKED, PICKER_WAITING,
    PhotosPickerProxy,
};
use porter_fake_servers::google::Pick;
use std::pin::Pin;
use std::time::Duration;
use syncd::datasets::photos::PhotosSwitch;
use syncd::datasets::storage::{FILES_APP, PHOTOS_APP};
use zbus::export::futures_core::Stream;

const DENIED: &str = "org.quire.Accounts1.Error.Denied";
const UNAVAILABLE: &str = "org.quire.Accounts1.Error.Unavailable";

fn pick(rig: &Rig, session: &str) {
    assert!(rig.google.picker_pick(
        session,
        vec![
            Pick {
                filename: "IMG_1.jpg".into(),
                bytes: vec![1, 2, 3],
                mime: "image/jpeg".into()
            },
            Pick {
                filename: "clip.mp4".into(),
                bytes: vec![9; 2000],
                mime: "video/mp4".into()
            },
        ],
    ));
}

async fn ready(photos: PhotosSwitch) -> Rig {
    let mut rig = rig(photos).await;
    serve_picker(&rig).await;
    rig.supervisor.tick().await;
    rig
}

/// Every message on the bus from the moment the monitor is set.
struct Tap(zbus::MessageStream);

impl Tap {
    async fn start(rig: &Rig) -> Self {
        let monitor = rig.bus.connect().await;
        zbus::fdo::MonitoringProxy::new(&monitor)
            .await
            .expect("proxy")
            .become_monitor(&[], 0)
            .await
            .expect("monitor");
        Self(zbus::MessageStream::from(monitor))
    }

    async fn drain(&mut self) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(300),
                std::future::poll_fn(|cx| Pin::new(&mut self.0).poll_next(cx)),
            )
            .await;
            match next {
                Ok(Some(Ok(message))) => seen.push(message.data().to_vec()),
                _ => return seen,
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_photos_app_starts_polls_imports_and_cancels_over_the_bus() {
    let rig = ready(PhotosSwitch::On).await;
    let mut tap = Tap::start(&rig).await;
    let app = client_of(&rig, PHOTOS_APP).await;
    let picker = PhotosPickerProxy::new(&app).await.expect("proxy");

    let (session, uri, poll_s) = picker.start(SEGMENT).await.expect("start");
    assert!(uri.contains(&session), "{uri}");
    assert_eq!(poll_s, 1);
    assert_eq!(
        picker.poll(SEGMENT, &session).await.expect("poll"),
        PICKER_WAITING
    );
    let early = picker.import(SEGMENT, &session).await.expect_err("not yet");
    assert_eq!(error_name(&early), PICKER_ERROR_NOT_YET);

    pick(&rig, &session);
    assert_eq!(
        picker.poll(SEGMENT, &session).await.expect("poll"),
        PICKER_PICKED
    );
    let files = picker.import(SEGMENT, &session).await.expect("import");
    let dir = rig.paths.photos_picked_dir(&rig.account).join(&session);
    assert_eq!(
        files,
        [dir.join("IMG_1.jpg"), dir.join("clip.mp4")].map(|p| p.to_string_lossy().into_owned())
    );
    assert_eq!(std::fs::read(&files[0]).expect("photo"), vec![1, 2, 3]);
    assert_eq!(std::fs::read(&files[1]).expect("video"), vec![9; 2000]);
    // The session is gone at Google.
    let gone = picker.poll(SEGMENT, &session).await.expect_err("gone");
    assert_eq!(error_name(&gone), PICKER_ERROR_NO_SUCH_SESSION);

    // Cancel ends another session without importing.
    let (second, _, _) = picker.start(SEGMENT).await.expect("start");
    picker.cancel(SEGMENT, &second).await.expect("cancel");
    assert!(rig.google.picker_sessions().iter().all(|s| s.deleted));
    assert!(
        !rig.paths
            .photos_picked_dir(&rig.account)
            .join(&second)
            .exists()
    );

    // The bus carried ids, a page, words and paths: no token, no bearer, no Google byte URL.
    let seen = tap.drain().await;
    assert!(
        seen.len() > 8,
        "the monitor saw the traffic: {}",
        seen.len()
    );
    for secret in [
        SECRET_ACCESS,
        SECRET_REFRESH,
        PICKER_BEARER,
        "Bearer ",
        "/dl/",
    ] {
        assert!(
            !seen.iter().any(|m| contains(m, secret)),
            "{secret} crossed the bus"
        );
    }
    assert!(
        seen.iter().any(|m| contains(m, &session)),
        "positive control: the scan finds a value that did cross"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_photos_app_may_call_and_every_bad_word_or_missing_picker_is_a_typed_error() {
    let rig = ready(PhotosSwitch::On).await;
    let app = client_of(&rig, PHOTOS_APP).await;
    let picker = PhotosPickerProxy::new(&app).await.expect("proxy");

    // Another app, Settings and a stranger.
    let files = client_of(&rig, FILES_APP).await;
    let other = PhotosPickerProxy::new(&files).await.expect("proxy");
    let refused = other.start(SEGMENT).await.expect_err("not the Photos app");
    assert_eq!(error_name(&refused), DENIED);
    let settings = common::client(
        &rig.bus,
        &rig.known,
        "org.quire.Settings",
        porter_dbus::CallerRole::Settings,
    )
    .await;
    let refused = PhotosPickerProxy::new(&settings)
        .await
        .expect("proxy")
        .start(SEGMENT)
        .await;
    assert_eq!(error_name(&refused.expect_err("settings")), DENIED);
    let stranger = rig.bus.connect().await;
    let refused = PhotosPickerProxy::new(&stranger)
        .await
        .expect("proxy")
        .start(SEGMENT)
        .await;
    assert_eq!(error_name(&refused.expect_err("stranger")), ACCESS_DENIED);

    // Words.
    for account in ["../x", "", "A B"] {
        let refused = picker.start(account).await.expect_err(account);
        assert_eq!(error_name(&refused), INVALID_ARGS, "{account:?}");
    }
    let refused = picker.poll(SEGMENT, "../x").await.expect_err("session");
    assert_eq!(error_name(&refused), INVALID_ARGS);
    let refused = picker.cancel(SEGMENT, "a b").await.expect_err("session");
    assert_eq!(error_name(&refused), INVALID_ARGS);

    // An account with no Picker.
    let refused = picker.start("nobody").await.expect_err("no account");
    assert_eq!(error_name(&refused), NO_FITTING);
    let refused = picker.import("nobody", "s1").await.expect_err("no account");
    assert_eq!(error_name(&refused), NO_FITTING);

    // A session Google does not have.
    let refused = picker
        .poll(SEGMENT, "s-unknown")
        .await
        .expect_err("unknown");
    assert_eq!(error_name(&refused), PICKER_ERROR_NO_SUCH_SESSION);

    // Google not answering is `Unavailable`, not a hang or a panic.
    let (session, _, _) = picker.start(SEGMENT).await.expect("start");
    // Dropping the fake asks its tasks to stop; a connection syncd keeps open may still be served
    // for a moment after (a poll then said "waiting", rel-13 follow-up), so ask until it is down.
    drop(rig.google);
    let deadline = tokio::time::Instant::now() + porter_fake::GENEROUS;
    let refused = loop {
        match picker.poll(SEGMENT, &session).await {
            Err(refused) => break refused,
            Ok(answer) => assert!(
                tokio::time::Instant::now() < deadline,
                "the fake Google still answered 60 s after it was dropped: {answer:?}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(error_name(&refused), UNAVAILABLE);
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_the_switch_off_there_is_no_picker_and_picker_only_consent_still_has_one() {
    // Off: Photos starts nothing, so no account has a Picker.
    let off = ready(PhotosSwitch::Off).await;
    let app = client_of(&off, PHOTOS_APP).await;
    let refused = PhotosPickerProxy::new(&app)
        .await
        .expect("proxy")
        .start(SEGMENT)
        .await
        .expect_err("off");
    assert_eq!(error_name(&refused), NO_FITTING);

    // The picker scope alone: the Picker stands, there is no upload.
    let mut only = rig_picker_only(PhotosSwitch::On).await;
    serve_picker(&only).await;
    only.supervisor.tick().await;
    let app = client_of(&only, PHOTOS_APP).await;
    let picker = PhotosPickerProxy::new(&app).await.expect("proxy");
    let (session, _, _) = picker.start(SEGMENT).await.expect("start");
    pick(&only, &session);
    assert_eq!(
        picker
            .import(SEGMENT, &session)
            .await
            .expect("import")
            .len(),
        2
    );
}
