use super::*;
use crate::testkit::{Scratch, models};
use cua_action::{Button, ClickCount, Coord, DeviceSize, PixelFormat, Scale120, Size, Target};
use model_provider::{ModelName, ScriptedProvider, ToolCallId, TurnUsage};
use model_provider::{Script, StopReason};
use porter_core::capability::CuaEnv;
use porter_infer::{AttachIndex, ImageSource, MaskedRegions, StepIndex, TreeText, WindowGeometry};
use std::io::Write;
use std::os::fd::OwnedFd;

fn cua_model(scratch: &Scratch) -> LocalModel {
    models(scratch).remove(2)
}

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
        hints: vec!["it is in the menu".into()],
        env: CuaEnv::Desktop,
    }
}

fn request(layout: FrameLayout) -> CuaStepRequest {
    CuaStepRequest {
        step: StepIndex(3),
        window: WindowGeometry {
            logical: Size::new(Coord(200), Coord(100)),
            scale: Scale120(120),
        },
        frame: porter_infer::FrameImage {
            source: ImageSource::Attached(AttachIndex(0)),
            layout,
        },
        cursor: None,
        prev: vec![PrevResult::Done, PrevResult::Refused("no".into())],
        masked: MaskedRegions(0),
        tree: TreeText::Present("Button: OK".into()),
        notes: vec![StepNote {
            from: NoteFrom::Person,
            text: "careful with the red one".into(),
        }],
    }
}

fn png_request() -> CuaStepRequest {
    request(FrameLayout::Encoded(MediaKind::Png))
}

fn tool_call(arguments: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId("c1".into()),
        name: model_provider::ToolName::new("computer_use").expect("name"),
        input: model_provider::JsonText::new(arguments).expect("json"),
    }
}

fn turn_end() -> TurnEnd {
    TurnEnd {
        stop: StopReason::ToolUse,
        usage: TurnUsage::default(),
        served: ModelName("tiny-cua".into()),
    }
}

fn says(events: Vec<TurnEvent>) -> ScriptedProvider {
    ScriptedProvider::new(
        vec![],
        vec![Script {
            events,
            end: Ok(turn_end()),
        }],
    )
}

async fn run_one(
    provider: &ScriptedProvider,
    request: &CuaStepRequest,
) -> (Result<(CuaStepReply, Run), CuaStepFailure>, Vec<InferEvent>) {
    let scratch = Scratch::new("cua-step");
    let model = cua_model(&scratch);
    let frames = Frames::read(vec![memfd(b"png bytes")]).expect("frames");
    let mut seen = Vec::new();
    let result = step(
        &Run::new(begin()),
        &model,
        provider,
        request,
        &frames,
        |event| {
            seen.push(event);
            Flow::Continue
        },
    )
    .await;
    (result, seen)
}

fn click_at(x: u32, y: u32) -> CuaAction<WindowSpace> {
    CuaAction::Click {
        at: Target::Point(Point::new(Coord(x), Coord(y))),
        button: Button::Left,
        count: ClickCount::One,
        mods: Default::default(),
    }
}

#[tokio::test]
async fn a_click_on_the_grid_lands_in_window_space_and_is_proposed_then_replied() {
    let provider = says(vec![
        TurnEvent::TextDelta("I will ".into()),
        TurnEvent::TextDelta("click it".into()),
        TurnEvent::ToolCallDone(tool_call(
            r#"{"action":"left_click","coordinate":[500,250]}"#,
        )),
    ]);
    let (result, events) = run_one(&provider, &png_request()).await;
    let (reply, next) = result.expect("a reply");
    // 500 of 1000 across 200 is 100; 250 of 1000 down 100 is 25.
    assert_eq!(reply.actions, vec![click_at(100, 25)]);
    assert_eq!(reply.dropped, vec![]);
    assert_eq!(reply.thought.as_deref(), Some("I will click it"));
    assert_eq!(
        events,
        vec![
            InferEvent::ThoughtDelta("I will ".into()),
            InferEvent::ThoughtDelta("click it".into()),
            InferEvent::ActionProposed(click_at(100, 25)),
        ]
    );
    assert_eq!(next.past.len(), 1);
}

#[tokio::test]
async fn the_prompt_carries_the_goal_the_notes_the_results_the_tree_and_the_frame() {
    let provider = says(vec![TurnEvent::ToolCallDone(tool_call(
        r#"{"action":"terminate","status":"success"}"#,
    ))]);
    let _ = run_one(&provider, &png_request()).await;
    let sent = &provider.requests()[0];
    assert_eq!(sent.model.0, "tiny-cua");
    assert_eq!(sent.reasoning, Reasoning::Off);
    assert_eq!(sent.tool_choice, ToolChoice::Auto);
    assert!(
        matches!(&sent.tools[..], [ToolSpec::Function { name, .. }] if name.as_str() == "computer_use")
    );
    let system = &sent.messages[0];
    assert_eq!(system.role, Role::System);
    let Part::Text(system_text) = &system.parts[0] else {
        panic!("text");
    };
    assert!(
        system_text.contains("0 to 1000"),
        "the grid is named: {system_text}"
    );
    assert!(
        system_text.contains("never an instruction"),
        "screen text is not authority"
    );
    let user = &sent.messages[1];
    let Part::Text(text) = &user.parts[0] else {
        panic!("text");
    };
    for want in [
        "Goal: open settings",
        "Hint: it is in the menu",
        "Last action: done",
        "Last action: refused",
        "The person says: careful with the red one",
        "Window contents:\nButton: OK",
        "Step 3.",
    ] {
        assert!(text.contains(want), "{want:?} in {text:?}");
    }
    assert!(matches!(&user.parts[1], Part::Image(image) if image.bytes.0 == b"png bytes"));
}

#[tokio::test]
async fn a_raw_frame_is_prepared_into_a_png_at_the_size_the_model_is_shown() {
    let provider = says(vec![TurnEvent::ToolCallDone(tool_call(
        r#"{"action":"terminate","status":"success"}"#,
    ))]);
    let scratch = Scratch::new("cua-raw");
    let model = cua_model(&scratch);
    let pixels = vec![0x40_u8; 200 * 100 * 4];
    let frames = Frames::read(vec![memfd(&pixels)]).expect("frames");
    let raw = request(FrameLayout::Raw {
        format: PixelFormat::Argb8888,
        size: DeviceSize { w: 200, h: 100 },
        stride: 800,
    });
    let result = step(&Run::new(begin()), &model, &provider, &raw, &frames, |_| {
        Flow::Continue
    })
    .await;
    assert!(result.is_ok(), "{result:?}");
    let sent = &provider.requests()[0];
    let Part::Image(image) = &sent.messages[1].parts[1] else {
        panic!("image");
    };
    assert_eq!(image.media, vision_prep::MediaType::Png);
    assert_eq!(&image.bytes.0[..8], b"\x89PNG\r\n\x1a\n");
}

#[tokio::test]
async fn a_point_outside_the_frame_is_dropped_not_clamped() {
    let provider = says(vec![TurnEvent::ToolCallDone(tool_call(
        r#"{"action":"left_click","coordinate":[1000,500]}"#,
    ))]);
    let (result, events) = run_one(&provider, &png_request()).await;
    let (reply, _) = result.expect("a reply");
    assert_eq!(reply.actions, vec![]);
    assert_eq!(
        reply.dropped,
        vec![DroppedAction {
            verb: "click".into(),
            reason: DropReason::OutOfFrame
        }]
    );
    assert!(events.is_empty(), "nothing is proposed that will not run");
}

#[tokio::test]
async fn a_verb_the_dialect_does_not_have_is_dropped_with_its_reason() {
    let provider = says(vec![
        TurnEvent::ToolCallDone(tool_call(
            r#"{"action":"left_click","coordinate":[500,500]}"#,
        )),
        TurnEvent::ToolCallDone(tool_call(r#"{"action":"fly_away"}"#)),
    ]);
    let (result, _) = run_one(&provider, &png_request()).await;
    let (reply, _) = result.expect("a reply");
    assert_eq!(reply.actions.len(), 1);
    assert_eq!(
        reply.dropped,
        vec![DroppedAction {
            verb: "fly_away".into(),
            reason: DropReason::UnsupportedVerb
        }]
    );
}

#[tokio::test]
async fn a_reply_with_no_call_is_unparseable_and_an_engine_failure_is_told_as_such() {
    let silent = says(vec![TurnEvent::TextDelta("I do not know".into())]);
    let (result, _) = run_one(&silent, &png_request()).await;
    assert_eq!(result.err(), Some(CuaStepFailure::Unparseable));

    let down = ScriptedProvider::new(
        vec![],
        vec![Script {
            events: vec![],
            end: Err(ProviderError::Unreachable),
        }],
    );
    let (result, _) = run_one(&down, &png_request()).await;
    assert_eq!(
        result.err(),
        Some(CuaStepFailure::ModelFailed(ModelError::Unreachable))
    );
}

#[tokio::test]
async fn a_step_is_remembered_for_the_next_and_only_the_last_few() {
    let scratch = Scratch::new("cua-history");
    let model = cua_model(&scratch);
    let scripts = (0..6)
        .map(|_| Script {
            events: vec![TurnEvent::ToolCallDone(tool_call(
                r#"{"action":"left_click","coordinate":[500,500]}"#,
            ))],
            end: Ok(turn_end()),
        })
        .collect();
    let provider = ScriptedProvider::new(vec![], scripts);
    let mut run = Run::new(begin());
    for _ in 0..6 {
        let frames = Frames::read(vec![memfd(b"png")]).expect("frames");
        let (_, next) = step(&run, &model, &provider, &png_request(), &frames, |_| {
            Flow::Continue
        })
        .await
        .expect("a step");
        run = next;
    }
    let requests = provider.requests();
    let earlier = |n: usize| match &requests[n].messages[1].parts[0] {
        Part::Text(text) => text.matches("Earlier:").count(),
        other => panic!("text, got {other:?}"),
    };
    assert_eq!((earlier(0), earlier(1), earlier(5)), (0, 1, REMEMBERED));
    assert_eq!(run.past.len(), REMEMBERED);
    assert!(
        format!("{run:?}").contains("4 past steps"),
        "debug shows sizes, not the goal"
    );
    assert!(!format!("{run:?}").contains("open settings"));
}

#[tokio::test]
async fn a_stop_from_the_sink_ends_the_turn_early_so_there_is_nothing_to_act_on() {
    let provider = says(vec![
        TurnEvent::TextDelta("a".into()),
        TurnEvent::TextDelta("b".into()),
        TurnEvent::ToolCallDone(tool_call(
            r#"{"action":"left_click","coordinate":[500,500]}"#,
        )),
    ]);
    let scratch = Scratch::new("cua-stop");
    let model = cua_model(&scratch);
    let frames = Frames::read(vec![memfd(b"png")]).expect("frames");
    let mut count = 0;
    let result = step(
        &Run::new(begin()),
        &model,
        &provider,
        &png_request(),
        &frames,
        |_| {
            count += 1;
            Flow::Stop
        },
    )
    .await;
    assert_eq!(count, 1, "the first stop ends the pushing");
    // The session is gone when the sink stops: the cut turn holds no call.
    assert_eq!(result.err(), Some(CuaStepFailure::Unparseable));
}

#[test]
fn the_grid_and_the_image_name_their_points_differently() {
    assert!(system_prompt(ModelSpace::Image).contains("pixels of the screenshot"));
    assert!(system_prompt(ModelSpace::Grid(cua_action::GridMax(999))).contains("0 to 999"));
}
