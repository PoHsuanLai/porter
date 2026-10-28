//! `AiKeys` against a hand-written accountd (`common::fake_accountd`): `RequestKey`
//! returns a Request object, and the answer is its `Response` signal (code 0 the answer, 1 closed,
//! 2 another refusal). The answer may beat the method's reply, and one from a connection that
//! does not own accountd's name is not heard. A client that gives up closes the sheet; one whose
//! daemon leaves is told nobody is there.
#![cfg(feature = "dbus")]

use crate::common;

use common::bus::PrivateBus;
use common::eventually;
use common::fake_accountd::{FakeAccountd, Sheets};
use porter_client::{AiKeys, AiKeysError, CallFailure};
use porter_core::DataClass;
use porter_core::consent::Usage;
use porter_wire::{ParentWindow, Refusal};
use std::sync::Arc;

/// A private bus with the fake on it, and a client connection.
struct Rig {
    bus: PrivateBus,
    daemon: zbus::Connection,
    fake: FakeAccountd,
    keys: AiKeys,
}

async fn rig(sheets: Sheets, forged: bool) -> Rig {
    let bus = PrivateBus::start();
    let daemon = bus.connect().await;
    let mut fake = FakeAccountd::new(sheets);
    if forged {
        fake = fake.forged_by(bus.connect().await);
    }
    fake.serve(&daemon).await;
    let keys = AiKeys::connect(&bus.connect().await).await.expect("keys");
    Rig {
        bus,
        daemon,
        fake,
        keys,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_request_is_answered_with_the_permit_even_when_the_answer_beats_the_reply_or_a_stranger_speaks_first()
 {
    for forged in [false, true] {
        let rig = rig(Sheets::Permit("permit-1"), forged).await;
        let permit = rig
            .keys
            .request_key(
                DataClass::Photos,
                Usage::Interactive,
                &ParentWindow::Handle("wayland:abc".into()),
            )
            .await
            .expect("the permit");
        assert_eq!(permit.as_str(), "permit-1", "forged: {forged}");
        assert_eq!(
            rig.fake.seen(|seen| seen.key_requests.clone()),
            [(
                "photos".to_owned(),
                "interactive".to_owned(),
                "wayland:abc".to_owned()
            )]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_way_a_sheet_can_end_is_a_typed_error() {
    let cases = [
        ("closed", Refusal::Dismissed),
        ("dont allow", Refusal::Denied),
        ("no key fits", Refusal::NoFittingAccount),
        ("sign in again", Refusal::NeedsReauth),
    ];
    for (name, refusal) in cases {
        let rig = rig(Sheets::Refuse(refusal), false).await;
        assert_eq!(
            rig.keys
                .request_key(
                    DataClass::Files,
                    Usage::Background,
                    &ParentWindow::Unparented
                )
                .await,
            Err(AiKeysError::Refused(refusal)),
            "{name}"
        );
    }
    // With no accountd on the bus there is nobody to ask.
    let bus = PrivateBus::start();
    let keys = AiKeys::connect(&bus.connect().await).await.expect("keys");
    assert_eq!(
        keys.request_key(
            DataClass::Files,
            Usage::Background,
            &ParentWindow::Unparented
        )
        .await,
        Err(AiKeysError::Bus(CallFailure::Unreachable))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_drops_the_call_takes_the_sheet_down() {
    let rig = rig(Sheets::Hang, false).await;
    let keys = Arc::new(rig.keys.clone());
    let waiting = {
        let keys = Arc::clone(&keys);
        tokio::spawn(async move {
            keys.request_key(
                DataClass::Photos,
                Usage::Interactive,
                &ParentWindow::Unparented,
            )
            .await
        })
    };
    eventually("the sheet to be asked for", || {
        rig.fake.seen(|seen| seen.key_requests.len()) == 1
    })
    .await;
    assert_eq!(rig.fake.seen(|seen| seen.closed), 0, "the sheet is open");

    waiting.abort();
    let _ = waiting.await;

    eventually("the sheet to be closed", || {
        rig.fake.seen(|seen| seen.closed) == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_leaves_mid_sheet_ends_the_wait() {
    let rig = rig(Sheets::Hang, false).await;
    let keys = rig.keys.clone();
    let waiting = tokio::spawn(async move {
        keys.request_key(
            DataClass::Photos,
            Usage::Interactive,
            &ParentWindow::Unparented,
        )
        .await
    });
    eventually("the sheet to be asked for", || {
        rig.fake.seen(|seen| seen.key_requests.len()) == 1
    })
    .await;

    rig.daemon.close().await.expect("the daemon leaves");

    let ended = tokio::time::timeout(test_kit::GENEROUS, waiting)
        .await
        .expect("the wait ends")
        .expect("task");
    assert_eq!(ended, Err(AiKeysError::Bus(CallFailure::Unreachable)));
    drop(rig.bus);
}
