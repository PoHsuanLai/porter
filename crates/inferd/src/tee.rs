//! A turn sink that keeps the whole turn for a pure session to read (stoker's `TranscriptSink`)
//! and shows the session's client what it is thinking while it waits.
//!
//! Both users of it, a validated structured reply and a computer-use step, must see the finished
//! turn before anything is said to the app (the first may be repaired, the second parsed), so
//! only the thoughts stream; what the turn says is told when it has been checked.

use cua_session::TranscriptSink;
use model_provider::{Flow, TurnEnd, TurnEvent, TurnSink};
use porter_infer::InferEvent;

/// What of the turn's text is shown as thinking while the turn runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    /// Thought deltas only (the text is a reply that is still to be validated).
    Thoughts,
    /// Thought deltas and text deltas (the text of a computer-use turn is the model's thinking
    /// out loud; its actions are the calls).
    ThoughtsAndText,
}

/// The sink: a transcript and a way to the session.
pub struct Tee<'a, F> {
    transcript: TranscriptSink,
    forward: &'a mut F,
    echo: Echo,
    last: Flow,
}

impl<F> std::fmt::Debug for Tee<'_, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tee({:?}, {:?})", self.echo, self.last)
    }
}

impl<'a, F: FnMut(InferEvent) -> Flow + Send> Tee<'a, F> {
    /// A sink that forwards to `forward` what `echo` allows.
    pub fn new(echo: Echo, forward: &'a mut F) -> Self {
        Self {
            transcript: TranscriptSink::new(),
            forward,
            echo,
            last: Flow::Continue,
        }
    }

    /// Whether the session asked to stop (nobody is listening any more).
    pub fn flow(&self) -> Flow {
        self.last
    }

    /// The finished turn.
    pub fn finish(self, end: TurnEnd) -> cua_session::TurnTranscript {
        self.transcript.finish(end)
    }
}

impl<F: FnMut(InferEvent) -> Flow + Send> TurnSink for Tee<'_, F> {
    fn event(&mut self, event: TurnEvent) -> Flow {
        let shown = match (&event, self.echo) {
            (TurnEvent::ThoughtDelta(text), _)
            | (TurnEvent::TextDelta(text), Echo::ThoughtsAndText) => {
                Some(InferEvent::ThoughtDelta(text.clone()))
            }
            _ => None,
        };
        let kept = self.transcript.event(event);
        let told = shown.map_or(Flow::Continue, |shown| (self.forward)(shown));
        self.last = match (kept, told) {
            (Flow::Continue, Flow::Continue) => Flow::Continue,
            _ => Flow::Stop,
        };
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_provider::{ModelName, StopReason, TurnUsage};

    fn end() -> TurnEnd {
        TurnEnd {
            stop: StopReason::EndTurn,
            usage: TurnUsage::default(),
            served: ModelName("m".into()),
        }
    }

    fn run(echo: Echo, stop_after: usize) -> (Vec<InferEvent>, Flow, cua_session::TurnTranscript) {
        let mut seen = Vec::new();
        let mut forward = |event| {
            seen.push(event);
            if seen.len() >= stop_after {
                Flow::Stop
            } else {
                Flow::Continue
            }
        };
        let mut tee = Tee::new(echo, &mut forward);
        tee.event(TurnEvent::ThoughtDelta("hm ".into()));
        tee.event(TurnEvent::TextDelta("say".into()));
        tee.event(TurnEvent::TextDelta("ing".into()));
        let flow = tee.flow();
        let transcript = tee.finish(end());
        (seen, flow, transcript)
    }

    #[test]
    fn the_transcript_keeps_everything_and_the_echo_shows_what_it_allows() {
        let (seen, flow, said) = run(Echo::Thoughts, usize::MAX);
        assert_eq!(seen, vec![InferEvent::ThoughtDelta("hm ".into())]);
        assert_eq!(
            (said.thought.as_str(), said.text.as_str()),
            ("hm ", "saying")
        );
        assert_eq!(flow, Flow::Continue);

        let (seen, _, _) = run(Echo::ThoughtsAndText, usize::MAX);
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[2], InferEvent::ThoughtDelta("ing".into()));
    }

    #[test]
    fn a_session_that_stops_stops_the_turn_and_the_sink_remembers_it() {
        let (seen, flow, said) = run(Echo::ThoughtsAndText, 1);
        assert_eq!(
            seen.len(),
            3,
            "the turn is told to stop and the sink keeps answering stop"
        );
        assert_eq!(flow, Flow::Stop);
        assert_eq!(said.text, "saying");
    }
}
