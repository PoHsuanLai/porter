//! `Start`, `Stop`, `Running` and `RunningChanged` of `Inference1`, over the real object with a
//! scripted engine behind the recording host.

use super::*;
use crate::hosting::bus::within;
use inferd::attached::{Attached, Place, Reach};
use porter_core::ModelId;
use porter_dbus::{
    Details, InferenceProxy, RunState, RunningModel, RunningRowError, running_from_dbus,
};
use std::time::{Duration, SystemTime};
use zbus::export::futures_core::Stream;

const CHAT: &str = "local/tiny-chat";

fn settings_world(script: Script) -> Plan {
    Plan {
        role: Role::Settings,
        ..chat_world(script)
    }
}

async fn proxy(world: &World) -> InferenceProxy<'_> {
    InferenceProxy::new(&world.client).await.expect("proxy")
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

fn read(rows: Vec<(String, Details)>) -> Result<Vec<RunningModel>, RunningRowError> {
    rows.into_iter().map(running_from_dbus).collect()
}

async fn running(inference: &InferenceProxy<'_>) -> Vec<RunningModel> {
    read(inference.running().await.expect("running")).expect("rows")
}

/// The one model of the world, once its engine is `state`.
async fn in_state(inference: &InferenceProxy<'_>, state: RunState) -> RunningModel {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        let rows = running(inference).await;
        if let Some(row) = rows.into_iter().find(|row| row.state == state) {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    deadline.fail("the engine to reach the state");
}

fn name_of(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, ..) => name.to_string(),
        other => format!("{other:?}"),
    }
}

fn words_of(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(words), _) => words.clone(),
        other => format!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn start_brings_a_scripted_engine_ready_and_stop_stops_it_and_both_are_told() {
    let began = std::time::Instant::now();
    let step = |what: &str| eprintln!("running test: {what} at {:?}", began.elapsed());
    let world = World::start(settings_world(Script::default())).await;
    step("world up");
    let inference = proxy(&world).await;
    let mut changed = inference.receive_running_changed().await.expect("signal");

    let stopped = running(&inference).await;
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    let row = &stopped[0];
    assert_eq!(
        (row.model.as_str(), row.state, row.engine.as_str()),
        (CHAT, RunState::Stopped, "llama-server")
    );
    assert!(row.memory_mib > 0 && row.since.is_none() && row.reason.is_none());

    inference.start(CHAT).await.expect("start");
    step("started");
    let ready = running(&inference).await.remove(0);
    assert_eq!(ready.state, RunState::Ready);
    let since = ready.since.expect("a ready engine says since when");
    let now = SystemTime::now();
    let off = now.duration_since(since).unwrap_or_else(|e| e.duration());
    assert!(
        off < Duration::from_secs(60),
        "since {since:?}, now {now:?}"
    );
    // The engine's window is chosen only for an engine started with one (the rig starts none).
    assert_eq!(ready.window, None);
    within("RunningChanged", next(&mut changed))
        .await
        .expect("an open stream");
    assert_eq!(world.host.spawned.lock().expect("lock").len(), 1);

    let mut after = inference.receive_running_changed().await.expect("signal");
    inference.stop(CHAT).await.expect("stop");
    step("stopped");
    let gone = in_state(&inference, RunState::Stopped).await;
    step("seen stopped");
    assert!(gone.since.is_none());
    within("RunningChanged after Stop", next(&mut after))
        .await
        .expect("an open stream");
    assert_eq!(world.host.stopped.lock().expect("lock").len(), 1);

    // Stopping a stopped model does nothing, and a later Start starts it again.
    inference.stop(CHAT).await.expect("stop again");
    step("stopped again");
    inference.start(CHAT).await.expect("start again");
    assert_eq!(world.host.spawned.lock().expect("lock").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_settings_may_start_stop_or_list() {
    let world = World::start(chat_world(Script::default())).await;
    let inference = proxy(&world).await;
    for refused in [
        inference.start(CHAT).await.expect_err("start"),
        inference.stop(CHAT).await.expect_err("stop"),
        inference.running().await.expect_err("running"),
    ] {
        assert!(name_of(&refused).ends_with("AccessDenied"), "{refused:?}");
    }
    assert!(world.host.spawned.lock().expect("lock").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_inferd_does_not_run_is_refused_in_plain_words() {
    let scratch = porter_fake::Scratch::new("run-attached");
    let world = World::start(Plan {
        role: Role::Settings,
        catalog: vec![("attached.toml", entries::attached())],
        attached: vec![Attached {
            id: ModelId::parse(entries::ATTACHED).expect("id"),
            reach: Reach::Socket(scratch.join("lab.sock")),
            key_file: None,
            place: Place::ThisDevice,
            computer: None,
        }],
        ..Plan::default()
    })
    .await;
    let inference = proxy(&world).await;
    let attached = format!("local/{}", entries::ATTACHED);
    let start = inference.start(&attached).await.expect_err("start");
    let stop = inference.stop(&attached).await.expect_err("stop");
    for refused in [start, stop] {
        assert!(name_of(&refused).ends_with("NotSupported"), "{refused:?}");
        assert!(words_of(&refused).contains("set up yourself"));
    }
    for unknown in ["local/nothing-here", "no-slash"] {
        let refused = inference.start(unknown).await.expect_err("start");
        assert!(name_of(&refused).ends_with("InvalidArgs"), "{refused:?}");
    }
    assert!(running(&inference).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_that_does_not_fit_fails_to_start_with_the_numbers_in_plain_words() {
    let big = entries::chat().replace("weights_mib = 100", "weights_mib = 20000");
    let world = World::start(Plan {
        role: Role::Settings,
        catalog: vec![("tiny-chat.toml", big)],
        ..Plan::default()
    })
    .await;
    let inference = proxy(&world).await;
    let refused = inference.start(CHAT).await.expect_err("no room");
    assert!(name_of(&refused).ends_with("Failed"), "{refused:?}");
    let words = words_of(&refused);
    assert!(
        words.starts_with("needs 19.") && words.contains(" GB of graphics memory; "),
        "{words}"
    );
    let failed = in_state(&inference, RunState::Failed).await;
    assert_eq!(failed.reason, Some(words));
    assert!(failed.since.is_none());
    assert!(world.host.spawned.lock().expect("lock").is_empty());
}
