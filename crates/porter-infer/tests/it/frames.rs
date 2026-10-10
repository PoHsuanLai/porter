//! The streaming session's frames, the computer-use step and the speech turn: every variant
//! survives its serde form and the forms inferd and its clients read keep their JSON.

use cua_action::{
    Button, ClickCount, Coord, CuaAction, DeviceSize, PixelFormat, Point, Scale120, Size, Target,
    WindowSpace,
};
use porter_core::capability::{CuaEnv, LanguageTag};
use porter_core::consent::Usage;
use porter_core::{AccountId, DataClass, Locality, ModelId, Permille, Tokens};
use porter_infer::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) -> String {
    let json = serde_json::to_string(value).expect("serializes");
    assert_eq!(
        &serde_json::from_str::<T>(&json).expect("deserializes"),
        value,
        "{json}"
    );
    json
}

fn served() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("holo-3.1-4b").expect("id"),
        locality: Locality::OnDevice,
    }
}

fn window_point(x: u32, y: u32) -> Point<WindowSpace> {
    Point::new(Coord(x), Coord(y))
}

fn click() -> CuaAction<WindowSpace> {
    CuaAction::Click {
        at: Target::Point(window_point(10, 20)),
        button: Button::Left,
        count: ClickCount::One,
        mods: Default::default(),
    }
}

fn step_request() -> CuaStepRequest {
    CuaStepRequest::new(
        StepIndex(3),
        WindowGeometry {
            logical: Size::new(Coord(1280), Coord(800)),
            scale: Scale120(180),
        },
        FrameImage {
            source: ImageSource::Attached(AttachIndex(0)),
            layout: FrameLayout::Raw {
                format: PixelFormat::Xrgb8888,
                size: DeviceSize { w: 1920, h: 1200 },
                stride: 7680,
            },
        },
        TreeText::Present("button \"Save\" [12]".into()),
    )
    .with_cursor(window_point(5, 6))
    .with_prev(vec![
        PrevResult::Done,
        PrevResult::Refused("outside the lease".into()),
        PrevResult::NotRun,
        PrevResult::Failed("no such button".into()),
        PrevResult::UserDeclined,
        PrevResult::UserActed,
    ])
    .with_masked(MaskedRegions(2))
    .with_notes(vec![
        StepNote {
            from: NoteFrom::Person,
            text: "also check the footer".into(),
        },
        StepNote {
            from: NoteFrom::Agent,
            text: "the file is in Downloads".into(),
        },
    ])
}

fn usage() -> TokenUsage {
    TokenUsage {
        input: Tokens(10),
        output: Tokens(5),
        cached: Tokens(0),
    }
}

#[test]
fn every_infer_request_round_trips() {
    let requests = vec![
        InferRequest::CuaBegin(
            CuaBegin::new("rename the file".into(), CuaEnv::Desktop)
                .with_hints(vec!["use the context menu".into()]),
        ),
        InferRequest::CuaStep(step_request()),
        InferRequest::Transcribe(TranscribeBegin::new(
            TranscribeMode::Streaming,
            LangPick::Prefer(vec![LanguageTag::parse("zh-Hant-TW").expect("tag")]),
            AudioRate(16_000),
            Usage::Interactive,
        )),
        InferRequest::Transcribe(TranscribeBegin::new(
            TranscribeMode::Batch,
            LangPick::Auto,
            AudioRate(16_000),
            Usage::Background,
        )),
        InferRequest::Speak(
            SpeakRequest::new(
                "Two new messages".into(),
                LanguageTag::parse("en").expect("tag"),
                DataClass::Mail,
                Usage::Interactive,
            )
            .with_voice(VoiceName("af_heart".into())),
        ),
    ];
    for request in &requests {
        round_trip(request);
    }
    let kinds: Vec<RequestKind> = requests.iter().map(InferRequest::kind).collect();
    assert_eq!(
        kinds,
        [
            RequestKind::CuaBegin,
            RequestKind::CuaStep,
            RequestKind::Transcribe,
            RequestKind::Transcribe,
            RequestKind::Speak
        ]
    );
}

#[test]
fn infer_request_speech_round_trip_pins_its_json() {
    let begin = InferRequest::Transcribe(TranscribeBegin::new(
        TranscribeMode::Streaming,
        LangPick::Auto,
        AudioRate(16_000),
        Usage::Interactive,
    ));
    assert_eq!(
        round_trip(&begin),
        r#"{"kind":"transcribe","v":{"mode":"streaming","lang":{"kind":"auto"},"rate":16000,"usage":"interactive"}}"#
    );
}

#[test]
fn client_frame_audio_round_trip() {
    let frames = [
        ClientFrame::Request(InferRequest::CuaStep(step_request())),
        ClientFrame::Cancel,
        ClientFrame::Audio(AudioFrame {
            at: 32_000,
            pcm: Base64Bytes(vec![0, 1, 2, 3]),
        }),
        ClientFrame::EndOfAudio,
    ];
    frames.iter().for_each(|f| {
        round_trip(f);
    });
    assert_eq!(round_trip(&ClientFrame::Cancel), r#"{"kind":"cancel"}"#);
    assert_eq!(
        round_trip(&frames[2]),
        r#"{"kind":"audio","v":{"at":32000,"pcm":"AAECAw=="}}"#
    );
}

#[test]
fn infer_frames_round_trip() {
    let events = vec![
        InferEvent::Routed(served()),
        InferEvent::Stage(StageNote {
            role: StageRole::Hear,
            served: served(),
            why: Why::Reached {
                provider: ProviderId("openrouter".into()),
                door: Door::Gateway,
            },
            name: Some(ModelLabel("Whisper".into())),
        }),
        InferEvent::Waiting(Readiness::Loading),
        InferEvent::Waiting(Readiness::Downloading(Permille(420))),
        InferEvent::TextDelta("he".into()),
        InferEvent::ThoughtDelta("hmm".into()),
        InferEvent::ToolCall(ToolCallPart {
            id: ToolCallId("c1".into()),
            name: ToolName::parse("mail.thread.archive").expect("name"),
            args: JsonText::parse("{}").expect("json"),
        }),
        InferEvent::ActionProposed(click()),
        InferEvent::Usage(usage()),
        InferEvent::Heard(HeardDelta::Partial {
            text: "hel".into(),
            from: 0,
        }),
        InferEvent::Heard(HeardDelta::Final {
            text: "hello".into(),
            from: 0,
            to: 16_000,
        }),
        InferEvent::Heard(HeardDelta::Lang(LanguageTag::parse("en").expect("tag"))),
        InferEvent::Spoken(AudioFrameOut {
            rate: AudioRate(24_000),
            at: 0,
            pcm: Base64Bytes(vec![9; 8]),
        }),
        InferEvent::Finished(InferReply::Cancelled),
    ];
    events.iter().for_each(|e| {
        round_trip(e);
    });
    let replies = [
        InferReply::CuaStep(
            CuaStepReply::new(vec![click(), CuaAction::Observe])
                .with_thought("click save".into())
                .with_dropped(vec![DroppedAction {
                    verb: "open_app".into(),
                    reason: DropReason::UnsupportedVerb,
                }])
                .with_safety(vec![SafetyHint::RequireConfirmation("sends money".into())]),
        ),
        InferReply::Transcribed(TranscribeReply::new("hello".into(), 1_000, served())),
        InferReply::Spoke(SpeakReply::new(2_000, served())),
        InferReply::Refused(InferRefusal::Unsupported),
        InferReply::Failed(ModelError::RateLimited(7)),
        InferReply::Cancelled,
    ];
    replies.iter().for_each(|r| {
        round_trip(r);
    });
    for failure in [
        CuaStepFailure::Unparseable,
        CuaStepFailure::ModelFailed(ModelError::ContextOverflow),
    ] {
        round_trip(&failure);
    }
}

#[test]
fn model_errors_and_refusals_keep_their_slugs() {
    let errors = [
        (ModelError::Unreachable, r#"{"kind":"unreachable"}"#),
        (
            ModelError::RateLimited(3),
            r#"{"kind":"rate_limited","v":3}"#,
        ),
        (ModelError::Unauthorized, r#"{"kind":"unauthorized"}"#),
        (
            ModelError::PaymentRequired,
            r#"{"kind":"payment_required"}"#,
        ),
        (ModelError::SignInRefused, r#"{"kind":"sign_in_refused"}"#),
        (ModelError::Refused, r#"{"kind":"refused"}"#),
        (ModelError::Unreadable, r#"{"kind":"unreadable"}"#),
        (ModelError::NotReady, r#"{"kind":"not_ready"}"#),
        (
            ModelError::ContextOverflow,
            r#"{"kind":"context_overflow"}"#,
        ),
        (ModelError::Unparseable, r#"{"kind":"unparseable"}"#),
        (
            ModelError::OnlyThought {
                stop: StopReason::EndTurn,
                thought_len: 12,
            },
            r#"{"kind":"only_thought","v":{"stop":"end_turn","thought_len":12}}"#,
        ),
    ];
    for (error, json) in errors {
        assert_eq!(round_trip(&error), json);
    }
    assert_eq!(
        round_trip(&InferRefusal::Unsupported),
        r#"{"kind":"unsupported"}"#
    );
}

#[test]
fn picker_data_round_trips_with_plain_slugs() {
    let model = ModelRef {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("nemotron").expect("id"),
    };
    let map = TierMap {
        rows: vec![TierRow {
            kind: Slot::VoiceIn,
            tier: porter_core::Tier::Balanced,
            model: model.clone(),
        }],
        autos: vec![],
    };
    round_trip(&map);
    let slugs: Vec<String> = [
        Slot::Text,
        Slot::ComputerUse,
        Slot::Embeddings,
        Slot::VoiceIn,
        Slot::VoiceOut,
        Slot::ImageGen,
        Slot::Rerank,
    ]
    .iter()
    .map(|kind| {
        let json = round_trip(kind);
        assert_eq!(json, format!("\"{}\"", kind.slug()));
        json
    })
    .collect();
    assert_eq!(slugs.len(), 7);
    // A frame written before the rename still reads, as the slot its kind became.
    assert_eq!(
        serde_json::from_str::<Slot>(r#""speech_in""#).ok(),
        Some(Slot::VoiceIn)
    );
    assert_eq!(
        serde_json::from_str::<Slot>(r#""llm""#).ok(),
        Some(Slot::Text)
    );
    round_trip(&PickerRow {
        model,
        label: "Nemotron streaming".into(),
        kind: Slot::VoiceIn,
        locality: Locality::OnDevice,
        billing: porter_core::Billing::Free,
        readiness: Readiness::Loadable,
        fit: Fit::Fits,
        licence: LicenceClass::Open,
        chosen_for: [porter_core::Tier::Balanced].into(),
    });
}

#[test]
fn readiness_slugs_match_prepare_answers() {
    let cases = [
        (Readiness::Ready, "ready"),
        (Readiness::Loading, "loading"),
        (Readiness::Loadable, "loadable"),
        (Readiness::Downloading(Permille(1)), "downloading"),
        (Readiness::Downloadable, "downloadable"),
        (Readiness::Unavailable, "unavailable"),
    ];
    for (readiness, slug) in cases {
        assert_eq!(readiness.slug(), slug);
        let json = round_trip(&readiness);
        assert!(json.contains(slug), "{json}");
    }
}

#[test]
fn debug_never_shows_what_the_person_said_or_saw() {
    let shown = [
        format!(
            "{:?}",
            CuaBegin::new("pay the plumber".into(), CuaEnv::Desktop)
        ),
        format!("{:?}", TreeText::Present("password: hunter2".into())),
        format!(
            "{:?}",
            SpeakRequest::new(
                "the plumber's number".into(),
                LanguageTag::parse("en").expect("tag"),
                DataClass::Contacts,
                Usage::Interactive,
            )
        ),
        format!(
            "{:?}",
            HeardDelta::Final {
                text: "pay the plumber".into(),
                from: 0,
                to: 1
            }
        ),
        format!(
            "{:?}",
            TranscribeReply::new("pay the plumber".into(), 1, served())
        ),
        format!(
            "{:?}",
            AudioFrame {
                at: 0,
                pcm: Base64Bytes(vec![42; 64])
            }
        ),
    ];
    for text in shown {
        for word in ["plumber", "hunter2", "42"] {
            assert!(!text.contains(word), "{text}");
        }
    }
}

#[test]
fn local_voice_floor_is_this_computer() {
    let policy = Policy::proposed();
    assert_eq!(policy.floor(DataClass::Voice), Floor::OnDevice);
    assert!(
        !policy
            .floor(DataClass::Voice)
            .admits(&Locality::Cloud { region: None })
    );
}

#[test]
fn every_data_class_has_a_floor_decision() {
    // The floors table is total: each class is either named in `proposed` or goes anywhere on
    // purpose. A new class must be added to one of the two lists below.
    let named_on_device = [
        DataClass::Mail,
        DataClass::Photos,
        DataClass::Notes,
        DataClass::Files,
        DataClass::Contacts,
        DataClass::Calendar,
        DataClass::Tasks,
        DataClass::Screen,
        DataClass::Clipboard,
        DataClass::Voice,
        DataClass::Prompt,
    ];
    let goes_anywhere = [DataClass::AppOwn, DataClass::Public];
    let policy = Policy::proposed();
    for class in named_on_device {
        assert_eq!(policy.floor(class), Floor::OnDevice, "{class:?}");
    }
    for class in goes_anywhere {
        assert_eq!(policy.floor(class), Floor::Anywhere, "{class:?}");
    }
    let total = |class: DataClass| match class {
        DataClass::AppOwn
        | DataClass::Mail
        | DataClass::Calendar
        | DataClass::Contacts
        | DataClass::Tasks
        | DataClass::Notes
        | DataClass::Files
        | DataClass::Photos
        | DataClass::Clipboard
        | DataClass::Screen
        | DataClass::Voice
        | DataClass::Prompt
        | DataClass::Public => (),
    };
    total(DataClass::Voice);
}

#[test]
fn a_scroll_with_no_coordinate_and_a_bad_argument_cross_the_wire() {
    // Ask 67: the model named nowhere, so the executor resolves the centre of the frame; and a
    // dropped action may say its argument was unusable. Both mirror stoker's cua-parse.
    let scroll = CuaAction::Scroll {
        at: Target::Centre,
        dir: cua_action::ScrollDir::Down,
        by: cua_action::ScrollBy::Notches(cua_action::Notches(3)),
    };
    let reply = CuaStepReply::new(vec![scroll]).with_dropped(vec![DroppedAction {
        verb: "key".into(),
        reason: DropReason::BadArgument,
    }]);
    let json = round_trip(&InferReply::CuaStep(reply));
    assert!(json.contains(r#""kind":"centre""#), "{json}");
    assert!(json.contains(r#""reason":"bad_argument""#), "{json}");
}

#[test]
fn step_notes_round_trip_pin_their_json_and_hide_their_text() {
    let note = StepNote {
        from: NoteFrom::Agent,
        text: "the file is in Downloads".into(),
    };
    assert_eq!(
        round_trip(&note),
        r#"{"from":"agent","text":"the file is in Downloads"}"#
    );
    assert_eq!(format!("{note:?}"), "StepNote(Agent, <24 bytes>)");
    assert!(!format!("{:?}", step_request()).contains("footer"));
}

#[test]
fn a_frame_names_how_many_descriptors_ride_with_it() {
    let image = |source| {
        MessagePart::Image(ImagePart {
            media_type: "image/png".into(),
            source,
        })
    };
    let chat = |parts| {
        ClientFrame::Request(InferRequest::Chat(
            ChatRequest::new(
                vec![ChatMessage {
                    role: Role::User,
                    parts,
                }],
                porter_core::Tier::Fast,
                DataClass::Public,
                Usage::Interactive,
            )
            .with_control(ChatControl::new().with_tool_calls(ToolParallelism::Many)),
        ))
    };
    let inline = || ImageSource::Inline(Base64Bytes(vec![1]));
    let at = |n| ImageSource::Attached(AttachIndex(n));
    let nested = MessagePart::ToolResult(ToolResultPart {
        id: ToolCallId("c".into()),
        status: ToolStatus::Ok,
        parts: vec![image(at(2))],
    });
    let cases = [
        ("no parts", chat(vec![]), 0),
        ("text only", chat(vec![MessagePart::Text("t".into())]), 0),
        ("inline is not a descriptor", chat(vec![image(inline())]), 0),
        ("one", chat(vec![image(at(0))]), 1),
        (
            "the highest index decides",
            chat(vec![image(at(0)), image(at(3))]),
            4,
        ),
        ("inside a tool result", chat(vec![nested]), 3),
        (
            "a computer-use frame",
            ClientFrame::Request(InferRequest::CuaStep(step_request())),
            1,
        ),
        ("cancel", ClientFrame::Cancel, 0),
        ("end of audio", ClientFrame::EndOfAudio, 0),
    ];
    for (name, frame, expected) in cases {
        assert_eq!(frame.attachments(), expected, "{name}");
    }
}

#[test]
fn why_reached_and_stage_notes_keep_their_json() {
    let why = Why::Reached {
        provider: ProviderId("openrouter".into()),
        door: Door::Gateway,
    };
    assert_eq!(
        round_trip(&why),
        r#"{"kind":"reached","provider":"openrouter","door":"gateway"}"#
    );
    assert_eq!(round_trip(&Why::Named), r#"{"kind":"named"}"#);
    round_trip(&StageNote {
        role: StageRole::Answer,
        served: served(),
        why: Why::Nearest,
        name: None,
    });
}

#[test]
fn a_stage_note_without_a_name_decodes_and_is_written_without_one() {
    let named = StageNote {
        role: StageRole::Answer,
        served: served(),
        why: Why::Nearest,
        name: Some(ModelLabel("Gemma 4".into())),
    };
    let json = serde_json::to_string(&named).expect("serializes");
    assert!(json.contains(r#""name":"Gemma 4""#), "{json}");
    // The payload an inferd that predates `name` writes: the same note without the key.
    let mut old = serde_json::to_value(&named).expect("value");
    old.as_object_mut().expect("object").remove("name");
    let note: StageNote = serde_json::from_value(old.clone()).expect("an old payload decodes");
    assert_eq!(note.name, None);
    assert_eq!(note.served, named.served);
    // A note without a name is written as that old payload, byte for byte.
    assert_eq!(serde_json::to_value(&note).expect("value"), old);
    assert!(!serde_json::to_string(&note).expect("json").contains("name"));
    // An old reader (no `name` field, unknown fields skipped) reads the new bytes.
    #[derive(serde::Deserialize)]
    struct Old {
        role: StageRole,
    }
    assert_eq!(
        serde_json::from_str::<Old>(&json).expect("old reader").role,
        StageRole::Answer
    );
}
