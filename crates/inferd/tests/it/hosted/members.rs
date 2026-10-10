//! The other members of `Inference1`, the introspection of the real object, and the failures a
//! session must survive.

use super::*;
use inferd::audit::Discard;
use inferd::clock::FixedClock;
use inferd::engines::Engines;
use inferd::peers::TablePeers;
use inferd::service::Inference;
use porter_core::UnixSeconds;
use porter_dbus::{Details, InferenceProxy, need_to_dbus};
use porter_infer::{InferRefusal, ModelError, Readiness};
use zbus::object_server::Interface;

async fn proxy(world: &World) -> InferenceProxy<'_> {
    InferenceProxy::new(&world.client).await.expect("proxy")
}

#[tokio::test(flavor = "multi_thread")]
async fn availability_prepare_and_the_gpu_answer_over_the_bus() {
    let world = World::start(chat_world(Script::default())).await;
    let inference = proxy(&world).await;
    let need = need_to_dbus(&llm());
    assert_eq!(
        inference
            .availability(&need, "notes", &Details::new())
            .await
            .expect("availability"),
        "granted"
    );
    // A class nobody has a model for under the floor, and a need nothing here can serve.
    let speech = need_to_dbus(&Need::Speech(porter_core::need::SpeechNeed::new(
        [porter_core::capability::SpeechMode::Stt].into(),
    )));
    assert_eq!(
        inference
            .availability(&speech, "voice", &Details::new())
            .await
            .expect("availability"),
        "needs_account"
    );

    // Warming a stopped engine says it is loading and starts it; asking again says ready.
    assert_eq!(inference.gpu().await.expect("gpu"), "idle");
    assert_eq!(
        inference
            .prepare(&need, "notes", "fast", &Details::new())
            .await
            .expect("prepare"),
        "loading"
    );
    let mut answer = String::new();
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        answer = inference
            .prepare(&need, "notes", "fast", &Details::new())
            .await
            .expect("prepare");
        if answer == Readiness::Ready.slug() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(answer, "ready");
    assert_eq!(world.host.spawned.lock().expect("lock").len(), 1);

    // A refusal answers with its slug.
    let cua = need_to_dbus(&cua_need());
    assert_eq!(
        inference
            .prepare(&cua, "screen", "fast", &Details::new())
            .await
            .expect("prepare"),
        "denied"
    );
    // Nothing meters yet; rescan only tells listeners to look again.
    assert!(inference.usage().await.expect("usage").is_empty());
    inference.rescan().await.expect("rescan");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_class_or_tier_is_an_invalid_argument() {
    let world = World::start(chat_world(Script::default())).await;
    let inference = proxy(&world).await;
    let need = need_to_dbus(&llm());
    for (class, tier) in [("gossip", "fast"), ("notes", "heroic")] {
        let got = inference.open(&need, class, tier, &Details::new()).await;
        assert!(
            matches!(&got, Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with("InvalidArgs")),
            "{class}/{tier}: {got:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_object_introspects_as_the_frozen_interface() {
    let object = Inference::new(
        Engines::default(),
        TablePeers::new(),
        Discard,
        FixedClock(UnixSeconds(0)),
    );
    let mut xml = String::from(
        "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\"\n \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n",
    );
    object.introspect_to_writer(&mut xml, 1);
    xml.push_str("</node>\n");
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dbus/org.quire.Inference1.xml");
    let expected = std::fs::read_to_string(&path).expect("the checked-in XML");
    assert_eq!(
        xml, expected,
        "inferd's object must declare exactly the frozen interface"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_engine_that_keeps_failing_is_retried_by_inferd_and_then_told_as_unreachable() {
    let world = World::start(chat_world(Script {
        chat: vec![Chat::Fail(503)],
        dims: 0,
    }))
    .await;
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Notes, Tier::Fast)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(chat_request("hi", DataClass::Notes)))
        .await
        .expect("send");
    let events = until_finished(&mut session).await;
    assert_eq!(
        events.last(),
        Some(&InferEvent::Finished(InferReply::Failed(
            ModelError::Unreachable
        )))
    );
    // Three attempts: inferd owns retry, the client never loops.
    assert_eq!(
        world.engines["tiny-chat"]
            .bodies("/v1/chat/completions")
            .len(),
        3
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_ends_the_running_turn_and_closes_the_engines_stream() {
    let world = World::start(chat_world(Script {
        chat: vec![Chat::Hang, Chat::Say(vec!["after"])],
        dims: 0,
    }))
    .await;
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Notes, Tier::Fast)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(chat_request(
            "slow one",
            DataClass::Notes,
        )))
        .await
        .expect("send");
    // Wait until the engine has the request, then change our mind.
    crate::support::eventually("the engine has the request", || {
        !world.engines["tiny-chat"]
            .bodies("/v1/chat/completions")
            .is_empty()
    })
    .await;
    session.send(ClientFrame::Cancel).await.expect("cancel");
    let events = until_finished(&mut session).await;
    assert_eq!(
        events.last(),
        Some(&InferEvent::Finished(InferReply::Cancelled))
    );
    // The session is still good: the next request is answered.
    session
        .send(ClientFrame::Request(chat_request(
            "another",
            DataClass::Notes,
        )))
        .await
        .expect("send");
    let events = until_finished(&mut session).await;
    assert!(
        matches!(events.last(), Some(InferEvent::Finished(InferReply::Chat(reply))) if reply.text == "after"),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_of_the_wrong_kind_for_the_session_is_refused_and_the_session_goes_on() {
    let world = World::start(chat_world(Script {
        chat: vec![Chat::Say(vec!["fine"])],
        dims: 0,
    }))
    .await;
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Notes, Tier::Fast)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(InferRequest::Embed(
            EmbedRequest::new(
                vec!["a".into()],
                EmbedRole::Query,
                DimsNeed::Any,
                DataClass::Notes,
                Usage::Interactive,
            ),
        )))
        .await
        .expect("send");
    let events = until_finished(&mut session).await;
    assert!(
        events.contains(&InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unsupported
        ))),
        "{events:?}"
    );
    session
        .send(ClientFrame::Request(chat_request(
            "now chat",
            DataClass::Notes,
        )))
        .await
        .expect("send");
    let events = until_finished(&mut session).await;
    assert!(matches!(
        events.last(),
        Some(InferEvent::Finished(InferReply::Chat(_)))
    ));
}
