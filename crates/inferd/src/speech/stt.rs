//! Speech to text over the speech host: [`SttBackend`] (the closed set of engines a `Transcribe`
//! turn can run on, one arm so far), the adapters between inferd's audio and event types and
//! stoker's, [`SpeechRunner::transcribe`], and [`Ears`], the `Transcriber` a pipeline's `Hear`
//! stage runs on.
//!
//! The host is a process of its own (`speech-host`, supervised as a CPU engine); one connection
//! carries one utterance. Cancelling a turn drops the future, which tells the host `Cancel` and
//! closes the socket, so audio is never kept: frames live in the buffer of the running call.

use super::{AudioIn, AudioPull, SpeechRunner};
use crate::bridge;
use crate::engines::Engines;
use crate::local::LocalModel;
use crate::pipeline::Transcriber;
use crate::serve::EngineHost;
use crate::supervise::Supervised;
use model_catalog::EngineKind;
use model_provider::{Flow as ProviderFlow, ModelName, ProviderError};
use porter_core::capability::LanguageTag;
use porter_infer::{
    ChatSink, Flow, HeardDelta, InferEvent, LangPick, ModelError, ModelRef, ServedBy, Stage,
    TranscribeBegin, TranscribeMode, TranscribeReply,
};
use speech_host_client::{HostSocket, SpeechHostClient};
use speech_provider::{
    AudioChunk, AudioFormat, AudioMs, AudioPull as SourcePull, AudioSource, Lang, LangChoice,
    PcmBytes, PcmFormat, SampleIndex, SampleRate, SpeechToText, SttEnd, SttMode, SttRequest,
    TranscriptEvent, TranscriptSink,
};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// How often a running turn tells the supervisor its engine is in use.
const TOUCH_EVERY: Duration = Duration::from_millis(250);

/// The step of a streaming engine whose catalogue entry does not say (`--chunk-ms`).
const DEFAULT_CHUNK: AudioMs = AudioMs(560);

/// The engines a speech-to-text turn can run on.
#[derive(Debug, Clone)]
pub enum SttBackend {
    /// `speech-host` on its Unix socket.
    SpeechHost(SpeechHostClient),
}

impl SttBackend {
    async fn transcribe(
        &self,
        request: &SttRequest,
        audio: &mut impl AudioSource,
        sink: &mut impl TranscriptSink,
    ) -> Result<SttEnd, ProviderError> {
        match self {
            SttBackend::SpeechHost(client) => client.transcribe(request, audio, sink).await,
        }
    }
}

/// What a model fixes about every utterance it hears: the name it is served under and the step
/// of its streaming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttShape {
    /// The catalogue id the host serves it under.
    pub model: ModelName,
    /// The size of the pieces it is fed, in a streaming turn.
    pub chunk: AudioMs,
}

impl SttShape {
    /// The model's: its catalogue id, and the `--chunk-ms` its engine profile passes.
    pub fn of(model: &LocalModel) -> Self {
        let args: Vec<&str> = model
            .profile
            .args
            .iter()
            .map(|arg| arg.0.as_str())
            .collect();
        let chunk = args
            .windows(2)
            .find(|pair| pair[0] == "--chunk-ms")
            .and_then(|pair| pair[1].parse().ok())
            .map_or(DEFAULT_CHUNK, AudioMs);
        Self {
            model: model.name.clone(),
            chunk,
        }
    }

    fn request(&self, begin: &TranscribeBegin) -> SttRequest {
        let lang = match &begin.lang {
            LangPick::Auto => LangChoice::Auto,
            LangPick::Prefer(tags) => {
                let langs: Vec<Lang> = tags
                    .iter()
                    .filter_map(|tag| Lang::new(String::from(tag.clone())).ok())
                    .collect();
                if langs.is_empty() {
                    LangChoice::Auto
                } else {
                    LangChoice::Prefer(langs)
                }
            }
        };
        SttRequest {
            model: self.model.clone(),
            mode: match begin.mode {
                TranscribeMode::Streaming => SttMode::Streaming { chunk: self.chunk },
                TranscribeMode::Batch => SttMode::Batch,
            },
            lang,
            format: AudioFormat {
                rate: SampleRate(begin.rate.0),
                pcm: PcmFormat::S16Le,
            },
        }
    }
}

impl SpeechRunner {
    /// The runner of one turn on `model` (an engine of kind speech host), answering as `served`
    /// and keeping the engine marked in use through `engines`. `None` when the model's engine
    /// is not a speech host.
    pub fn for_model(model: &LocalModel, served: ServedBy, engines: Supervised) -> Option<Self> {
        (model.profile.kind == EngineKind::SpeechHost).then(|| Self {
            backend: SttBackend::SpeechHost(SpeechHostClient::new(HostSocket(
                model.socket.0.clone(),
            ))),
            request: SttShape::of(model),
            served,
            engines,
            engine: model.spec.id.clone(),
        })
    }

    /// A speech-to-text turn: audio in, `Heard` events into `sink`, the transcript out. The
    /// audio is read as the host asks for it and ends when `audio` does (the person stopped
    /// talking); a sink answering `Stop` ends the turn early with what was heard so far.
    pub async fn transcribe(
        &self,
        begin: &TranscribeBegin,
        audio: &mut impl AudioIn,
        sink: &mut impl ChatSink,
    ) -> Result<TranscribeReply, ModelError> {
        let request = self.request.request(begin);
        let touch = Touch::new(&self.engines, &self.engine);
        let mut source = Source {
            audio,
            format: request.format,
            touch: touch.clone(),
        };
        let mut heard = Heard { sink, touch };
        match self
            .backend
            .transcribe(&request, &mut source, &mut heard)
            .await
        {
            Ok(end) => Ok(TranscribeReply::new(
                end.text.0,
                end.audio.0,
                self.served.clone(),
            )),
            Err(error) => Err(bridge::model_error(&error)),
        }
    }
}

/// Tells the supervisor, no more than every [`TOUCH_EVERY`], that the engine is in use.
#[derive(Debug, Clone)]
struct Touch {
    engines: Supervised,
    engine: engine_supervisor::EngineId,
    last: Option<Instant>,
}

impl Touch {
    fn new(engines: &Supervised, engine: &engine_supervisor::EngineId) -> Self {
        Self {
            engines: engines.clone(),
            engine: engine.clone(),
            last: None,
        }
    }

    fn used(&mut self) {
        if self.last.is_none_or(|last| last.elapsed() >= TOUCH_EVERY) {
            self.last = Some(Instant::now());
            self.engines.used(&self.engine);
        }
    }
}

/// inferd's audio as the host client's source: each checked frame becomes one chunk.
struct Source<'a, A> {
    audio: &'a mut A,
    format: AudioFormat,
    touch: Touch,
}

impl<A: AudioIn> AudioSource for Source<'_, A> {
    async fn next(&mut self) -> SourcePull {
        self.touch.used();
        match self.audio.next().await {
            AudioPull::Frame(frame) => SourcePull::Chunk(AudioChunk {
                format: self.format,
                at: SampleIndex(frame.at),
                pcm: PcmBytes::new(frame.pcm.0.clone()),
            }),
            AudioPull::End => SourcePull::End,
        }
    }
}

/// The host's transcript events as the wire's `Heard` events.
struct Heard<'a, S> {
    sink: &'a mut S,
    touch: Touch,
}

impl<S: ChatSink> TranscriptSink for Heard<'_, S> {
    fn event(&mut self, event: TranscriptEvent) -> ProviderFlow {
        self.touch.used();
        let Some(delta) = heard(event) else {
            return ProviderFlow::Continue;
        };
        match self.sink.event(InferEvent::Heard(delta)) {
            Flow::Continue => ProviderFlow::Continue,
            Flow::Stop => ProviderFlow::Stop,
        }
    }
}

/// One transcript event on the wire; a language the wire cannot spell is dropped.
fn heard(event: TranscriptEvent) -> Option<HeardDelta> {
    Some(match event {
        TranscriptEvent::Partial { text, from } => HeardDelta::Partial {
            text: text.0,
            from: from.0,
        },
        TranscriptEvent::Final { text, from, to } => HeardDelta::Final {
            text: text.0,
            from: from.0,
            to: to.0,
        },
        TranscriptEvent::Lang(lang) => HeardDelta::Lang(LanguageTag::parse(lang.as_str()).ok()?),
    })
}

/// The audio of a `Transcribe` turn as it arrives: the session machine's checked frames go in
/// through the sender, the end of audio is the sender being dropped.
#[derive(Debug)]
pub struct ChannelAudio(pub(crate) mpsc::UnboundedReceiver<porter_infer::AudioFrame>);

impl AudioIn for ChannelAudio {
    async fn next(&mut self) -> AudioPull {
        match self.0.recv().await {
            Some(frame) => AudioPull::Frame(frame),
            None => AudioPull::End,
        }
    }
}

/// The ears of a pipeline: a `Hear` stage runs on the speech host of the model the stage chose
/// (the host is asked for first, so a stopped engine is started and waited for).
#[derive(Debug, Clone)]
pub struct Ears {
    engines: Engines,
}

impl Ears {
    /// Ears over these engines.
    pub fn new(engines: Engines) -> Self {
        Self { engines }
    }
}

impl Transcriber for Ears {
    async fn transcribe<A: AudioIn, S: ChatSink>(
        &self,
        stage: &Stage,
        begin: &TranscribeBegin,
        audio: &mut A,
        sink: &mut S,
    ) -> Result<TranscribeReply, ModelError> {
        let model = ModelRef {
            account: stage.picked.chosen.account.clone(),
            model: stage.picked.chosen.model.clone(),
        };
        let Some(local) = self.engines.local_model(&model) else {
            return Err(ModelError::Unreachable);
        };
        let Some(runner) =
            SpeechRunner::for_model(&local, stage.served(), self.engines.supervised().clone())
        else {
            return Err(ModelError::Unreachable);
        };
        self.engines
            .want(model)
            .await
            .map_err(|_| ModelError::Unreachable)?;
        runner.transcribe(begin, audio, sink).await
    }
}

#[cfg(test)]
mod tests;
