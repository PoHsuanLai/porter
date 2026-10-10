use super::*;
use porter_core::consent::Usage;
use porter_core::{AccountId, DataClass, Locality, ModelId, Tier};
use porter_infer::{
    AttachIndex, Base64Bytes, ChatMessage, ChatRequest, CuaBegin, CuaStepRequest, FrameImage,
    FrameLayout, ImagePart, ImageSource, LangPick, MediaKind, Role, ServedBy, SpeakReply,
    SpeakRequest, StepIndex, ToolCallId, ToolResultPart, ToolStatus, TranscribeBegin,
    TranscribeMode, TranscribeReply, TreeText, WindowGeometry,
};

fn served() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("m").expect("id"),
        locality: Locality::OnDevice,
    }
}

fn image() -> MessagePart {
    MessagePart::Image(ImagePart {
        media_type: "image/png".into(),
        source: ImageSource::Attached(AttachIndex(0)),
    })
}

fn message(parts: Vec<MessagePart>) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        parts,
    }
}

fn chat(messages: Vec<ChatMessage>) -> InferRequest {
    InferRequest::Chat(ChatRequest::new(
        messages,
        Tier::Fast,
        DataClass::Notes,
        Usage::Interactive,
    ))
}

fn tool_result(parts: Vec<MessagePart>) -> MessagePart {
    MessagePart::ToolResult(ToolResultPart {
        id: ToolCallId("c".into()),
        status: ToolStatus::Ok,
        parts,
    })
}

fn step() -> InferRequest {
    InferRequest::CuaStep(CuaStepRequest::new(
        StepIndex(0),
        WindowGeometry {
            logical: cua_action::Size::new(cua_action::Coord(1), cua_action::Coord(1)),
            scale: cua_action::Scale120(120),
        },
        FrameImage {
            source: ImageSource::Attached(AttachIndex(0)),
            layout: FrameLayout::Encoded(MediaKind::Png),
        },
        TreeText::Absent,
    ))
}

fn transcribe(rate: u32) -> InferRequest {
    InferRequest::Transcribe(TranscribeBegin::new(
        TranscribeMode::Batch,
        LangPick::Auto,
        AudioRate(rate),
        Usage::Interactive,
    ))
}

fn frame(at: u64, samples: usize) -> AudioFrame {
    AudioFrame {
        at,
        pcm: Base64Bytes(vec![0; samples * 2]),
    }
}

#[test]
fn the_images_a_request_carries_are_counted_wherever_they_sit() {
    let nested = tool_result(vec![image(), MessagePart::Text("t".into()), image()]);
    let cases: Vec<(&str, InferRequest, u32)> = vec![
        (
            "no image",
            chat(vec![message(vec![MessagePart::Text("hi".into())])]),
            0,
        ),
        ("one", chat(vec![message(vec![image()])]), 1),
        (
            "across messages",
            chat(vec![
                message(vec![image(), image()]),
                message(vec![image()]),
            ]),
            3,
        ),
        ("inside a tool result", chat(vec![message(vec![nested])]), 2),
        ("a computer-use step is one frame", step(), 1),
        (
            "a begin carries none",
            InferRequest::CuaBegin(CuaBegin::new(
                "g".into(),
                porter_core::capability::CuaEnv::Desktop,
            )),
            0,
        ),
        ("a transcription carries none", transcribe(16_000), 0),
    ];
    for (name, request, images) in cases {
        let carried = Tally::begin(&request).closing(&InferReply::Cancelled);
        assert_eq!(carried.images, Count(images), "{name}");
    }
}

#[test]
fn audio_sent_is_milliseconds_at_the_turns_own_rate_and_adds_up_across_frames() {
    let cases = [
        ("none", 16_000, vec![], 0),
        ("one second", 16_000, vec![16_000], 1_000),
        ("two frames", 16_000, vec![8_000, 4_000], 750),
        ("a part of a millisecond is dropped", 16_000, vec![15], 0),
        ("another rate", 8_000, vec![8_000], 1_000),
    ];
    for (name, rate, frames, ms) in cases {
        let mut tally = Tally::begin(&transcribe(rate));
        let mut at = 0;
        for samples in frames {
            tally.heard(&frame(at, samples));
            at += samples as u64;
        }
        assert_eq!(
            tally.closing(&InferReply::Cancelled).audio_ms,
            Count(ms),
            "{name}"
        );
    }
}

#[test]
fn audio_produced_is_added_from_the_reply_and_a_transcript_is_not_counted_twice() {
    let spoke = InferReply::Spoke(SpeakReply::new(1_200, served()));
    let speak = InferRequest::Speak(SpeakRequest::new(
        "hi".into(),
        porter_core::capability::LanguageTag::parse("en").expect("tag"),
        DataClass::Voice,
        Usage::Interactive,
    ));
    assert_eq!(Tally::begin(&speak).closing(&spoke).audio_ms, Count(1_200));

    let transcribed =
        InferReply::Transcribed(TranscribeReply::new("words".into(), 1_000, served()));
    let mut tally = Tally::begin(&transcribe(16_000));
    tally.heard(&frame(0, 16_000));
    assert_eq!(tally.closing(&transcribed).audio_ms, Count(1_000));
}

#[test]
fn a_turn_that_failed_still_reports_what_it_carried() {
    let mut tally = Tally::begin(&transcribe(16_000));
    tally.heard(&frame(0, 1_600));
    let failed = InferReply::Failed(porter_infer::ModelError::Unreadable);
    assert_eq!(
        tally.closing(&failed),
        Carried {
            images: Count(0),
            audio_ms: Count(100)
        }
    );
}
