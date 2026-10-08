//! The scripted session: transcript events arrive as audio passes their index, a request with
//! no script is refused, Cancel ends the turn.

use porter_core::capability::LanguageTag;
use porter_core::consent::Usage;
use porter_core::{DataClass, Tokens};
use porter_fake::{FakeInferSession, Script, ScriptStep};
use porter_infer::{
    AudioFrame, AudioRate, Base64Bytes, ClientFrame, HeardDelta, InferEvent, InferRefusal,
    InferReply, InferRequest, InferSession, LangPick, RequestKind, SessionError, SpeakRequest,
    TokenUsage, TranscribeBegin, TranscribeMode,
};

fn transcribe() -> InferRequest {
    InferRequest::Transcribe(TranscribeBegin {
        mode: TranscribeMode::Streaming,
        lang: LangPick::Auto,
        rate: AudioRate(16_000),
        usage: Usage::Interactive,
    })
}

fn audio(samples: usize, at: u64) -> ClientFrame {
    ClientFrame::Audio(AudioFrame {
        at,
        pcm: Base64Bytes(vec![0; samples * 2]),
    })
}

fn heard(text: &str) -> HeardDelta {
    HeardDelta::Partial {
        text: text.into(),
        from: 0,
    }
}

#[tokio::test]
async fn a_transcript_arrives_as_the_audio_passes_its_index() {
    let script = Script {
        kind: RequestKind::Transcribe,
        steps: vec![
            ScriptStep::AfterAudio {
                samples: 16_000,
                event: InferEvent::Heard(heard("hel")),
            },
            ScriptStep::AfterAudio {
                samples: 32_000,
                event: InferEvent::Heard(heard("hello")),
            },
        ],
    };
    let mut session = FakeInferSession::scripted([script]);
    session
        .send(ClientFrame::Request(transcribe()))
        .await
        .expect("send");
    session.send(audio(8_000, 0)).await.expect("send");
    assert_eq!(session.next().await, Err(SessionError::Closed), "not yet");
    session.send(audio(8_000, 8_000)).await.expect("send");
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Heard(heard("hel"))),
        "the first index passed"
    );
    session.send(ClientFrame::EndOfAudio).await.expect("send");
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Heard(heard("hello"))),
        "end of audio releases the rest"
    );
    assert_eq!(session.sent().len(), 4);
}

#[tokio::test]
async fn a_request_without_a_script_is_refused_and_cancel_ends_a_turn() {
    let speak = InferRequest::Speak(SpeakRequest {
        text: "hi".into(),
        voice: None,
        lang: LanguageTag::parse("en").expect("tag"),
        class: DataClass::Public,
        usage: Usage::Interactive,
    });
    let script = Script {
        kind: RequestKind::Chat,
        steps: vec![ScriptStep::Emit(InferEvent::Usage(TokenUsage {
            input: Tokens(1),
            output: Tokens(1),
            cached: Tokens(0),
        }))],
    };
    let mut session = FakeInferSession::scripted([script]);
    session
        .send(ClientFrame::Request(speak))
        .await
        .expect("send");
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unsupported
        )))
    );
    session.send(ClientFrame::Cancel).await.expect("send");
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(InferReply::Cancelled))
    );
}
