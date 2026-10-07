//! The client against the real session server: `Open` over a private bus returns a socket that
//! `inferd::serve` serves with the real session machine over scripted seams. What the machine
//! adds to the scripts (`Routed`, `Waiting`, refusals, the computer-use begin) reaches the client
//! over the bus exactly as inferd will send it.
#![cfg(all(feature = "dbus", feature = "infer"))]

mod common;

use common::bus::PrivateBus;
use common::inferd::{Behaviour, FakeInferd, Served};
use common::served::{Gate, Route, Scripted};
use inferd::session::RouteDecision;
use porter_client::{Accounts, ClientError, DbusTransport, InferSession};
use porter_core::capability::CuaEnv;
use porter_core::consent::Usage;
use porter_core::need::{CuaNeed, DimsNeed, EmbedNeed};
use porter_core::{AccountId, DataClass, Dims, Locality, ModelId, Need, Tier, Tokens};
use porter_fake::{Script, ScriptStep};
use porter_infer::{
    AttachIndex, ClientFrame, CuaBegin, CuaStepReply, EmbedReply, EmbedRequest, EmbedRole,
    EmbedVector, ImageSource, InferEvent, InferRefusal, InferReply, InferRequest, Readiness,
    RequestKind, ServedBy, TokenUsage,
};
use std::sync::Arc;
use tokio::sync::Notify;

fn served_by() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("qwen").expect("id"),
        locality: Locality::OnDevice,
    }
}

fn decision(readiness: Readiness) -> Route {
    Route(Ok(RouteDecision {
        served: served_by(),
        readiness,
    }))
}

fn usage() -> TokenUsage {
    TokenUsage {
        input: Tokens(1),
        output: Tokens(0),
        cached: Tokens(0),
    }
}

struct Rig {
    _bus: PrivateBus,
    _daemon: zbus::Connection,
    accounts: Accounts<DbusTransport>,
    runner: Scripted,
}

async fn rig(route: Route, gate: Gate, scripts: Vec<Script>) -> Rig {
    let bus = PrivateBus::start();
    let runner = Scripted::new(scripts);
    let (fake, _seen) = FakeInferd::new(Behaviour::Served(Served {
        route,
        gate,
        runner: runner.clone(),
    }));
    let daemon = bus.connect().await;
    fake.serve(&daemon).await;
    Rig {
        accounts: Accounts::over(DbusTransport::over(bus.connect().await)),
        _bus: bus,
        _daemon: daemon,
        runner,
    }
}

fn embeddings() -> Need {
    Need::Embeddings(EmbedNeed {
        dims: DimsNeed::Exactly(Dims(2)),
        modalities: [porter_core::capability::Modality::Text].into(),
    })
}

fn embed_request() -> InferRequest {
    InferRequest::Embed(EmbedRequest {
        inputs: vec!["a note".into()],
        role: EmbedRole::Document,
        dims: DimsNeed::Exactly(Dims(2)),
        class: DataClass::Notes,
        usage: Usage::Background,
    })
}

fn embed_reply() -> InferReply {
    InferReply::Embed(EmbedReply {
        vectors: vec![EmbedVector(vec![0.5, 0.25])],
        usage: usage(),
        served: served_by(),
    })
}

fn embed_script() -> Script {
    Script {
        kind: RequestKind::Embed,
        steps: vec![ScriptStep::Emit(InferEvent::Finished(embed_reply()))],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn memoryds_embedding_turn_runs_through_the_real_machine_and_says_who_answered() {
    let rig = rig(
        decision(Readiness::Ready),
        Gate::default(),
        vec![embed_script()],
    )
    .await;
    let mut session = rig
        .accounts
        .session(&embeddings(), DataClass::Notes, Tier::Fast)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(embed_request()))
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Routed(served_by())));
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(embed_reply()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_one_call_form_memoryd_uses_returns_the_reply() {
    let rig = rig_again(vec![embed_script()]).await;
    let got = rig
        .accounts
        .infer(&embeddings(), DataClass::Notes, Tier::Fast, embed_request())
        .await;
    assert_eq!(got, Ok(embed_reply()));
}

async fn rig_again(scripts: Vec<Script>) -> Rig {
    rig(decision(Readiness::Ready), Gate::default(), scripts).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_class_the_session_was_not_opened_for_is_refused_by_the_machine() {
    let rig = rig_again(vec![embed_script()]).await;
    let mut session = rig
        .accounts
        .session(&embeddings(), DataClass::Public, Tier::Fast)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(embed_request()))
        .await
        .expect("send");
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unsupported
        ))),
        "the request carries Notes on a Public session"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_loading_engine_is_a_waiting_event_until_it_is_ready() {
    let gate = Arc::new(Notify::new());
    let rig = rig(
        decision(Readiness::Loadable),
        Gate(Some(Arc::clone(&gate))),
        vec![embed_script()],
    )
    .await;
    let mut session = rig
        .accounts
        .session(&embeddings(), DataClass::Notes, Tier::Fast)
        .await
        .expect("open");
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Waiting(Readiness::Loadable))
    );
    session
        .send(ClientFrame::Request(embed_request()))
        .await
        .expect("send");
    gate.notify_one();
    assert_eq!(session.next().await, Ok(InferEvent::Routed(served_by())));
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(embed_reply()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_refusal_is_what_infer_returns() {
    let rig = rig(
        Route(Err(InferRefusal::NeedsGrant)),
        Gate::default(),
        vec![],
    )
    .await;
    let got = rig
        .accounts
        .infer(&embeddings(), DataClass::Notes, Tier::Fast, embed_request())
        .await;
    assert_eq!(
        got,
        Err(ClientError::InferRefused(InferRefusal::NeedsGrant))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_use_run_needs_its_begin_and_its_frame_rides_as_a_memfd() {
    let ack = InferReply::CuaStep(CuaStepReply {
        thought: None,
        actions: vec![],
        dropped: vec![],
        safety: vec![],
    });
    let step_reply = InferReply::CuaStep(CuaStepReply {
        thought: Some("done".into()),
        actions: vec![cua_action::CuaAction::Observe],
        dropped: vec![],
        safety: vec![],
    });
    let scripts = vec![
        Script {
            kind: RequestKind::CuaBegin,
            steps: vec![ScriptStep::Emit(InferEvent::Finished(ack.clone()))],
        },
        Script {
            kind: RequestKind::CuaStep,
            steps: vec![ScriptStep::Emit(InferEvent::Finished(step_reply.clone()))],
        },
    ];
    let rig = rig_again(scripts).await;
    let need = Need::ComputerUse(CuaNeed {
        environments: [CuaEnv::Desktop].into(),
    });
    let mut session = rig
        .accounts
        .session(&need, DataClass::Screen, Tier::Best)
        .await
        .expect("open");
    let frame =
        || rustix::fs::memfd_create("frame", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd");
    let step = ClientFrame::Request(InferRequest::CuaStep(step_request()));

    session
        .send_attached(step.clone(), vec![frame()])
        .await
        .expect("send");
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unsupported
        ))),
        "a step before a begin"
    );

    session
        .send(ClientFrame::Request(InferRequest::CuaBegin(CuaBegin {
            goal: "rename".into(),
            hints: vec![],
            env: CuaEnv::Desktop,
        })))
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Routed(served_by())));
    assert_eq!(session.next().await, Ok(InferEvent::Finished(ack)));

    session
        .send_attached(step, vec![frame()])
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Finished(step_reply)));
    let seen = rig.runner.seen.lock().expect("lock");
    let kinds: Vec<(RequestKind, usize)> = seen.iter().map(|(r, n)| (r.kind(), *n)).collect();
    assert_eq!(
        kinds,
        vec![(RequestKind::CuaBegin, 0), (RequestKind::CuaStep, 1)],
        "the engine saw the begin, then the step with its one descriptor"
    );
}

fn step_request() -> porter_infer::CuaStepRequest {
    porter_infer::CuaStepRequest {
        step: porter_infer::StepIndex(0),
        window: porter_infer::WindowGeometry {
            logical: cua_action::Size::new(cua_action::Coord(1), cua_action::Coord(1)),
            scale: cua_action::Scale120(120),
        },
        frame: porter_infer::FrameImage {
            source: ImageSource::Attached(AttachIndex(0)),
            layout: porter_infer::FrameLayout::Encoded(porter_infer::MediaKind::Png),
        },
        cursor: None,
        prev: vec![],
        masked: porter_infer::MaskedRegions(0),
        tree: porter_infer::TreeText::Absent,
        notes: vec![],
    }
}
