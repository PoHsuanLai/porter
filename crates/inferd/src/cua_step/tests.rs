use super::*;
use crate::testkit::{Scratch, models};
use cua_action::{
    Button, ClickCount, Coord, CuaAction, DeviceSize, PixelFormat, Point, Scale120, Size, Target,
    WindowSpace,
};
use model_provider::{
    EngineExtras, ModelName, ProviderError, Script, ScriptedProvider, StopReason, ToolCall,
    ToolCallId as CallId, TurnEnd, TurnEvent, TurnUsage,
};
use porter_core::capability::CuaEnv;
use porter_infer::{AttachIndex, ImageSource, MaskedRegions as Masked, StepIndex as Index};
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
        step: Index(3),
        window: porter_infer::WindowGeometry {
            logical: Size::new(Coord(200), Coord(100)),
            scale: Scale120(120),
        },
        frame: porter_infer::FrameImage {
            source: ImageSource::Attached(AttachIndex(0)),
            layout,
        },
        cursor: None,
        prev: vec![PrevResult::Done, PrevResult::Refused("no".into())],
        masked: Masked(0),
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
        id: CallId("c1".into()),
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

fn script(events: Vec<TurnEvent>) -> Script {
    Script {
        events,
        end: Ok(turn_end()),
    }
}

fn click(arguments: &str) -> TurnEvent {
    TurnEvent::ToolCallDone(tool_call(arguments))
}

fn says(events: Vec<TurnEvent>) -> ScriptedProvider {
    ScriptedProvider::new(vec![], vec![script(events)])
}

fn session(model: &LocalModel) -> CuaSession {
    open(model, &begin()).expect("a session")
}

type Stepped = Result<(CuaStepReply, CuaSession), Failed>;

async fn run_one(
    provider: &ScriptedProvider,
    request: &CuaStepRequest,
) -> (Stepped, Vec<InferEvent>) {
    let scratch = Scratch::new("cua-step");
    let model = cua_model(&scratch);
    let frames = Frames::read(vec![memfd(b"png bytes")]).expect("frames");
    let mut seen = Vec::new();
    let result = step(
        session(&model),
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

fn texts(turn: &TurnRequest, role: Role) -> Vec<String> {
    turn.messages
        .iter()
        .filter(|m| m.role == role)
        .flat_map(|m| m.parts.iter())
        .filter_map(|part| match part {
            Part::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn images(turn: &TurnRequest) -> usize {
    turn.messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter(|part| matches!(part, Part::Image(_)))
        .count()
}

#[tokio::test]
async fn a_click_on_the_grid_lands_in_window_space_and_is_proposed_then_replied() {
    let provider = says(vec![
        TurnEvent::TextDelta("I will ".into()),
        TurnEvent::TextDelta("click it".into()),
        click(r#"{"action":"left_click","coordinate":[500,250]}"#),
    ]);
    let (result, events) = run_one(&provider, &png_request()).await;
    let (reply, kept) = result.expect("a reply");
    assert_eq!(reply.actions, vec![click_at(100, 25)]);
    assert_eq!(reply.thought.as_deref(), Some("I will click it"));
    assert_eq!(
        events,
        vec![
            InferEvent::ThoughtDelta("I will ".into()),
            InferEvent::ThoughtDelta("click it".into()),
            InferEvent::ActionProposed(click_at(100, 25)),
        ]
    );
    assert_eq!(kept.remembered(), 1);
}

#[tokio::test]
async fn the_prompt_carries_the_goal_the_results_the_notes_the_tree_and_the_frame() {
    let provider = says(vec![click(r#"{"action":"terminate","status":"success"}"#)]);
    let _ = run_one(&provider, &png_request()).await;
    let sent = &provider.requests()[0];
    assert_eq!(sent.model.0, "tiny-cua");
    let system = texts(sent, Role::System).join("\n");
    assert!(system.contains("1000"), "the grid is named: {system}");
    let user = texts(sent, Role::User).join("\n");
    for want in [
        "open settings",
        "it is in the menu",
        "refused (no)",
        "The person says: careful with the red one",
        "Window contents:\nButton: OK",
        "Step 3",
    ] {
        assert!(user.contains(want), "{want:?} in {user:?}");
    }
    let last = sent.messages.last().expect("the step's message");
    assert_eq!(last.role, Role::User);
    assert!(
        matches!(last.parts.last(), Some(Part::Image(image)) if image.bytes.0 == b"png bytes"),
        "the frame is last"
    );
}

#[tokio::test]
async fn the_request_is_set_from_the_catalog_entry() {
    let provider = says(vec![click(r#"{"action":"terminate","status":"success"}"#)]);
    let _ = run_one(&provider, &png_request()).await;
    let sent = &provider.requests()[0];
    let scratch = Scratch::new("cua-settings");
    let model = cua_model(&scratch);
    let entry = model.entry.sampling.expect("sampling");
    assert_eq!(
        sent.sampling, entry.reasoning_off,
        "greedy: the same screen, the same click"
    );
    assert_eq!(sent.reasoning, model_provider::Reasoning::Off);
    assert_eq!(
        sent.limits.max_output,
        model.caps().expect("caps").max_output
    );
    assert_eq!(sent.tool_calls, ToolParallelism::One);
    assert_eq!(sent.engine, EngineExtras::None, "vLLM has no extras");
}

#[tokio::test]
async fn a_raw_frame_is_prepared_into_a_png_at_the_size_the_model_is_shown() {
    let provider = says(vec![click(r#"{"action":"terminate","status":"success"}"#)]);
    let scratch = Scratch::new("cua-raw");
    let model = cua_model(&scratch);
    let pixels = vec![0x40_u8; 200 * 100 * 4];
    let frames = Frames::read(vec![memfd(&pixels)]).expect("frames");
    let raw = request(FrameLayout::Raw {
        format: PixelFormat::Argb8888,
        size: DeviceSize { w: 200, h: 100 },
        stride: 800,
    });
    let result = step(session(&model), &model, &provider, &raw, &frames, |_| {
        Flow::Continue
    })
    .await;
    assert!(result.is_ok(), "{:?}", result.err());
    let sent = &provider.requests()[0];
    let Some(Part::Image(image)) = sent.messages.last().and_then(|m| m.parts.last()) else {
        panic!("image");
    };
    assert_eq!(image.media, vision_prep::MediaType::Png);
    assert_eq!(&image.bytes.0[..8], b"\x89PNG\r\n\x1a\n");
}

#[tokio::test]
async fn a_point_outside_the_frame_is_dropped_not_clamped_after_one_repair() {
    let off = || {
        script(vec![click(
            r#"{"action":"left_click","coordinate":[1000,500]}"#,
        )])
    };
    // Every action refused is answered with a repair; when that fails too, the refusals are told.
    let provider = ScriptedProvider::new(vec![], vec![off(), off()]);
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
    assert_eq!(provider.requests().len(), 2);

    // An action that does land beside the one that does not is kept, with no repair.
    let mixed = says(vec![
        click(r#"{"action":"left_click","coordinate":[1000,500]}"#),
        click(r#"{"action":"left_click","coordinate":[500,250]}"#),
    ]);
    let (result, _) = run_one(&mixed, &png_request()).await;
    let (reply, _) = result.expect("a reply");
    assert_eq!(reply.actions, vec![click_at(100, 25)]);
    assert_eq!(reply.dropped.len(), 1);
    assert_eq!(mixed.requests().len(), 1);
}

#[tokio::test]
async fn a_verb_the_dialect_does_not_have_is_dropped_with_its_reason() {
    let provider = says(vec![
        click(r#"{"action":"left_click","coordinate":[500,500]}"#),
        click(r#"{"action":"fly_away"}"#),
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
async fn a_reply_that_does_not_parse_is_asked_again_once_with_the_same_frame() {
    let provider = ScriptedProvider::new(
        vec![],
        vec![
            script(vec![TurnEvent::TextDelta("I do not know".into())]),
            script(vec![click(
                r#"{"action":"left_click","coordinate":[500,250]}"#,
            )]),
        ],
    );
    let (result, _) = run_one(&provider, &png_request()).await;
    let (reply, kept) = result.expect("the repaired reply");
    assert_eq!(reply.actions, vec![click_at(100, 25)]);
    let requests = provider.requests();
    assert_eq!(requests.len(), 2, "one turn, one repair turn");
    assert_eq!(
        requests[1].messages.len(),
        requests[0].messages.len() + 1,
        "the repair is the first request and one message"
    );
    assert_eq!(
        images(&requests[0]),
        images(&requests[1]),
        "the same frame, no new one"
    );
    let said = texts(&requests[1], Role::User).join("\n");
    assert!(
        !said.contains("I do not know"),
        "the reply is not repeated: {said}"
    );
    assert_eq!(kept.remembered(), 1, "one step, however many turns it took");
}

#[tokio::test]
async fn a_reply_that_never_parses_is_unparseable_and_the_step_still_counts() {
    let provider = ScriptedProvider::new(
        vec![],
        vec![
            script(vec![TurnEvent::TextDelta("hm".into())]),
            script(vec![TurnEvent::TextDelta("hmm".into())]),
        ],
    );
    let (result, _) = run_one(&provider, &png_request()).await;
    let Err(Failed { failure, kept }) = result else {
        panic!("a failure");
    };
    assert_eq!(failure, CuaStepFailure::Unparseable);
    assert_eq!(kept.map(|session| session.remembered()), Some(1));
    assert_eq!(provider.requests().len(), 2);
}

#[tokio::test]
async fn an_engine_failure_is_told_as_such_and_the_step_does_not_count() {
    let down = ScriptedProvider::new(
        vec![],
        vec![Script {
            events: vec![],
            end: Err(ProviderError::Unreachable),
        }],
    );
    let (result, _) = run_one(&down, &png_request()).await;
    let Err(Failed { failure, kept }) = result else {
        panic!("a failure");
    };
    assert_eq!(
        failure,
        CuaStepFailure::ModelFailed(ModelError::Unreachable)
    );
    assert!(kept.is_none());
}

#[tokio::test]
async fn the_prompt_holds_one_frame_fewer_than_the_model_takes_and_lists_the_steps() {
    let scratch = Scratch::new("cua-history");
    let model = cua_model(&scratch);
    let per_prompt = model.caps().expect("caps").images.per_prompt.0;
    assert_eq!(per_prompt, 3);
    let scripts = (0..5)
        .map(|_| {
            script(vec![click(
                r#"{"action":"left_click","coordinate":[500,500]}"#,
            )])
        })
        .collect();
    let provider = ScriptedProvider::new(vec![], scripts);
    let mut current = session(&model);
    for _ in 0..5 {
        let frames = Frames::read(vec![memfd(b"png")]).expect("frames");
        let (_, next) = step(current, &model, &provider, &png_request(), &frames, |_| {
            Flow::Continue
        })
        .await
        .expect("a step");
        current = next;
    }
    let requests = provider.requests();
    let held: Vec<usize> = requests.iter().map(images).collect();
    assert_eq!(
        held,
        vec![1, 2, 3, 3, 3],
        "the images of a prompt never pass {per_prompt}"
    );
    assert_eq!(current.remembered(), 5);
}

#[tokio::test]
async fn a_stop_from_the_sink_ends_the_turn_so_there_is_nothing_to_act_on_and_no_repair() {
    let provider = ScriptedProvider::new(
        vec![],
        vec![
            script(vec![
                TurnEvent::TextDelta("a".into()),
                TurnEvent::TextDelta("b".into()),
                click(r#"{"action":"left_click","coordinate":[500,500]}"#),
            ]),
            script(vec![click(
                r#"{"action":"left_click","coordinate":[500,500]}"#,
            )]),
        ],
    );
    let scratch = Scratch::new("cua-stop");
    let model = cua_model(&scratch);
    let frames = Frames::read(vec![memfd(b"png")]).expect("frames");
    let mut count = 0;
    let result = step(
        session(&model),
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
    assert_eq!(
        result.err().map(|failed| failed.failure),
        Some(CuaStepFailure::Unparseable)
    );
    assert_eq!(provider.requests().len(), 1, "nobody is asked for a repair");
}

#[test]
fn the_history_is_one_frame_fewer_than_the_prompt_takes() {
    let table = [(0, 0), (1, 0), (2, 1), (3, 2), (256, 255), (1000, 255)];
    for (per_prompt, history) in table {
        assert_eq!(history_of(per_prompt), FrameBudget(history), "{per_prompt}");
    }
}

#[test]
fn a_session_opens_for_a_tool_or_text_dialect_and_for_nothing_else() {
    let scratch = Scratch::new("cua-open");
    let mut models = models(&scratch);
    assert!(open(&models[2], &begin()).is_some());
    assert!(
        open(&models[0], &begin()).is_none(),
        "a chat model has no dialect"
    );
    models[2].entry.caps.as_mut().expect("caps").computer_use = CuaSupport::Dialect {
        dialect: CuaDialect::Wire(cua_action::WireDialect::OpenAiComputer),
        batching: Batching::One,
        zoom: model_provider::Zoom::Absent,
    };
    assert!(open(&models[2], &begin()).is_none());
}

#[test]
fn what_became_of_the_actions_is_told_in_stokers_three_words() {
    let id = |n: usize| CallId(format!("prev-{n}"));
    let got = prev_results(&[
        PrevResult::Done,
        PrevResult::Refused("no".into()),
        PrevResult::NotRun,
        PrevResult::Failed("gone".into()),
        PrevResult::UserDeclined,
        PrevResult::UserActed,
    ]);
    assert_eq!(
        got,
        vec![
            StepResult::Done(id(0)),
            StepResult::Refused {
                id: id(1),
                why: "no".into()
            },
            StepResult::NotRun(id(2)),
            StepResult::Refused {
                id: id(3),
                why: "gone".into()
            },
            StepResult::Refused {
                id: id(4),
                why: "the person declined it".into()
            },
            StepResult::Refused {
                id: id(5),
                why: "the person did it themselves".into()
            },
        ]
    );
}

#[test]
fn the_context_goes_after_the_lead_and_before_the_frames() {
    let mut turn = TurnRequest {
        model: ModelName("m".into()),
        messages: vec![
            model_provider::Message {
                role: Role::System,
                parts: vec![Part::Text("system".into())],
            },
            model_provider::Message {
                role: Role::User,
                parts: vec![Part::Text("lead".into()), Part::Text("frame label".into())],
            },
        ],
        tools: vec![],
        tool_choice: model_provider::ToolChoice::Auto,
        tool_calls: ToolParallelism::One,
        output: model_provider::OutputShape::Free,
        limits: Limits {
            max_output: model_provider::Tokens(1),
            stop: vec![],
        },
        sampling: model_provider::Sampling {
            temperature: model_provider::Milli(0),
            top_p: model_provider::Knob::Off,
            top_k: model_provider::Knob::Off,
            min_p: model_provider::Knob::Off,
            repeat_penalty: model_provider::Knob::Off,
            seed: model_provider::Knob::Off,
        },
        reasoning: model_provider::Reasoning::Off,
        engine: EngineExtras::None,
    };
    add_context(&mut turn, "context".into());
    let order: Vec<String> = texts(&turn, Role::User);
    assert_eq!(order, ["lead", "context", "frame label"]);
    assert_eq!(
        texts(&turn, Role::System),
        ["system"],
        "the system turn is untouched"
    );
}

#[test]
fn no_notes_and_no_tree_add_nothing() {
    let mut bare = png_request();
    bare.notes.clear();
    bare.tree = TreeText::Absent;
    assert_eq!(context_text(&bare), None);
    let only_notes = CuaStepRequest {
        tree: TreeText::Absent,
        ..png_request()
    };
    assert_eq!(
        context_text(&only_notes).as_deref(),
        Some("The person says: careful with the red one")
    );
}
