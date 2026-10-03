use super::*;
use crate::testkit::{Scratch, models};
use cua_action::{
    Button, ClickCount, Coord, CuaAction, Point, Scale120, Size, Target, WindowSpace,
};
use model_provider::{
    ModelName, Part, Script, ScriptedProvider, StopReason, ToolCall, ToolCallId, TurnEnd,
    TurnEvent, TurnUsage,
};
use porter_core::capability::CuaEnv;
use porter_infer::{
    AttachIndex, FrameImage, FrameLayout, InferEvent, MaskedRegions, MediaKind, StepIndex,
    TreeText, WindowGeometry,
};
use std::io::Write;
use std::os::fd::OwnedFd;

fn memfd(bytes: &[u8]) -> OwnedFd {
    let fd = rustix::fs::memfd_create("t", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd");
    std::fs::File::from(fd.try_clone().expect("dup"))
        .write_all(bytes)
        .expect("fill");
    fd
}

fn begin() -> CuaBegin {
    CuaBegin {
        goal: "open settings".into(),
        hints: vec![],
        env: CuaEnv::Desktop,
    }
}

fn request(step: u32) -> CuaStepRequest {
    CuaStepRequest {
        step: StepIndex(step),
        window: WindowGeometry {
            logical: Size::new(Coord(200), Coord(100)),
            scale: Scale120(120),
        },
        frame: FrameImage {
            source: porter_infer::ImageSource::Attached(AttachIndex(0)),
            layout: FrameLayout::Encoded(MediaKind::Png),
        },
        cursor: None,
        prev: vec![],
        masked: MaskedRegions(0),
        tree: TreeText::Absent,
        notes: vec![],
    }
}

fn click(arguments: &str) -> TurnEvent {
    TurnEvent::ToolCallDone(ToolCall {
        id: ToolCallId("c".into()),
        name: model_provider::ToolName::new("computer_use").expect("name"),
        input: model_provider::JsonText::new(arguments).expect("json"),
    })
}

fn script(events: Vec<TurnEvent>) -> Script {
    Script {
        events,
        end: Ok(TurnEnd {
            stop: StopReason::ToolUse,
            usage: TurnUsage::default(),
            served: ModelName("tiny-cua".into()),
        }),
    }
}

fn at(x: u32, y: u32) -> CuaAction<WindowSpace> {
    CuaAction::Click {
        at: Target::Point(Point::new(Coord(x), Coord(y))),
        button: Button::Left,
        count: ClickCount::One,
        mods: Default::default(),
    }
}

/// Collects what a step streams, and stops after `stop_after` events if it says so.
#[derive(Default)]
struct Events {
    seen: Vec<InferEvent>,
    stop_after: Option<usize>,
}

impl ChatSink for Events {
    fn event(&mut self, event: InferEvent) -> Flow {
        self.seen.push(event);
        match self.stop_after {
            Some(n) if self.seen.len() >= n => Flow::Stop,
            _ => Flow::Continue,
        }
    }
}

struct Rig {
    scratch: Scratch,
}

impl Rig {
    fn new() -> Self {
        Self {
            scratch: Scratch::new("cua-run"),
        }
    }

    async fn step(
        &self,
        run: &mut CuaRun,
        provider: &ScriptedProvider,
        number: u32,
        sink: &mut Events,
    ) -> Result<CuaStepReply, CuaStepFailure> {
        let model = models(&self.scratch).remove(2);
        let frames = Frames::read(vec![memfd(b"png bytes")]).expect("frames");
        let request = request(number);
        run.step(
            StepJob {
                model: &model,
                provider,
                frames: &frames,
                request: &request,
            },
            sink,
        )
        .await
    }
}

#[tokio::test]
async fn a_step_streams_its_thought_and_proposals_into_the_sink_and_returns_the_reply() {
    let provider = ScriptedProvider::new(
        vec![],
        vec![script(vec![
            TurnEvent::TextDelta("clicking".into()),
            click(r#"{"action":"left_click","coordinate":[500,250]}"#),
        ])],
    );
    let mut sink = Events::default();
    let reply = Rig::new()
        .step(&mut CuaRun::begin(begin()), &provider, 0, &mut sink)
        .await
        .expect("a reply");
    assert_eq!(reply.actions, vec![at(100, 25)]);
    assert_eq!(
        sink.seen,
        vec![
            InferEvent::ThoughtDelta("clicking".into()),
            InferEvent::ActionProposed(at(100, 25)),
        ]
    );
}

fn user_text(provider: &ScriptedProvider, which: usize) -> String {
    let sent = &provider.requests()[which];
    match &sent.messages[1].parts[0] {
        Part::Text(text) => text.clone(),
        other => panic!("text, got {other:?}"),
    }
}

#[tokio::test]
async fn the_run_remembers_its_steps_and_a_failed_step_is_not_one_of_them() {
    let provider = ScriptedProvider::new(
        vec![],
        vec![
            script(vec![click(
                r#"{"action":"left_click","coordinate":[500,500]}"#,
            )]),
            // The second answer has no call: unparseable.
            script(vec![TurnEvent::TextDelta("hm".into())]),
            script(vec![click(
                r#"{"action":"left_click","coordinate":[100,100]}"#,
            )]),
        ],
    );
    let rig = Rig::new();
    let mut run = CuaRun::begin(begin());
    let mut sink = Events::default();
    rig.step(&mut run, &provider, 0, &mut sink)
        .await
        .expect("the first");
    assert_eq!(
        rig.step(&mut run, &provider, 1, &mut sink).await.err(),
        Some(CuaStepFailure::Unparseable)
    );
    rig.step(&mut run, &provider, 1, &mut sink)
        .await
        .expect("the same step again");

    assert!(!user_text(&provider, 0).contains("Earlier:"));
    assert!(user_text(&provider, 1).contains("Earlier: you did: click"));
    let third = user_text(&provider, 2);
    assert_eq!(
        third.matches("Earlier:").count(),
        1,
        "the failed step was not remembered: {third}"
    );
}

#[tokio::test]
async fn a_sink_that_stops_ends_the_proposals_but_not_the_reply() {
    let provider = ScriptedProvider::new(
        vec![],
        vec![script(vec![
            click(r#"{"action":"left_click","coordinate":[500,500]}"#),
            click(r#"{"action":"left_click","coordinate":[100,100]}"#),
        ])],
    );
    let mut sink = Events {
        stop_after: Some(1),
        ..Events::default()
    };
    let reply = Rig::new()
        .step(&mut CuaRun::begin(begin()), &provider, 0, &mut sink)
        .await
        .expect("a reply");
    assert_eq!(sink.seen.len(), 1, "one proposal, then Stop");
    assert_eq!(
        reply.actions.len(),
        2,
        "the reply still holds what was parsed"
    );
}

#[test]
fn only_screen_data_enters_a_computer_use_session() {
    use porter_core::DataClass;
    assert_eq!(check_class(DataClass::Screen), Ok(()));
    for class in [
        DataClass::Mail,
        DataClass::Voice,
        DataClass::Public,
        DataClass::AppOwn,
    ] {
        assert_eq!(
            check_class(class),
            Err(InferRefusal::Unsupported),
            "{class:?}"
        );
    }
}
