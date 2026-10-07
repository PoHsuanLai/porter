//! Stoker's answers as porter's: `TurnEvent` to `InferEvent`, a turn's end to a `ChatReply`,
//! `ProviderError` to `ModelError`, `StopReason` both ways. The only place the two vocabularies
//! meet on the way out.

use model_provider as sp;
use porter_core::{Dims, Tokens};
use porter_infer as pi;

/// What a turn has produced so far, folded into the reply it ends with.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Gathered {
    text: String,
    thought: String,
    calls: Vec<pi::ToolCallPart>,
}

/// The event a stoker event is, if the client sees it: deltas, finished tool calls and usage
/// (usage is metering that rides on `Finished`, so the client sees it only there).
pub fn event(event: &sp::TurnEvent) -> Option<pi::InferEvent> {
    match event {
        sp::TurnEvent::TextDelta(text) => Some(pi::InferEvent::TextDelta(text.clone())),
        sp::TurnEvent::ThoughtDelta(text) => Some(pi::InferEvent::ThoughtDelta(text.clone())),
        sp::TurnEvent::ToolCallDone(call) => call_part(call).map(pi::InferEvent::ToolCall),
        sp::TurnEvent::ThoughtSealed(_)
        | sp::TurnEvent::ToolCallStarted { .. }
        | sp::TurnEvent::ToolCallDelta { .. }
        | sp::TurnEvent::Safety(_)
        | sp::TurnEvent::Usage(_) => None,
    }
}

/// A finished call as the wire has it. A name stoker accepted is always one of ours, and its
/// arguments are checked JSON, so this answers `None` only for a name outside our grammar.
fn call_part(call: &sp::ToolCall) -> Option<pi::ToolCallPart> {
    Some(pi::ToolCallPart {
        id: pi::ToolCallId(call.id.0.clone()),
        name: pi::ToolName::parse(call.name.as_str()).ok()?,
        args: pi::JsonText::parse(call.input.as_str()).ok()?,
    })
}

impl Gathered {
    /// Takes one event into the reply being built.
    pub fn take(&mut self, event: &sp::TurnEvent) {
        match event {
            sp::TurnEvent::TextDelta(text) => self.text.push_str(text),
            sp::TurnEvent::ThoughtDelta(text) => self.thought.push_str(text),
            sp::TurnEvent::ToolCallDone(call) => self.calls.extend(call_part(call)),
            _ => {}
        }
    }

    /// The reply the turn ended with.
    pub fn chat_reply(self, end: &sp::TurnEnd, served: pi::ServedBy) -> pi::ChatReply {
        pi::ChatReply {
            text: self.text,
            tool_calls: self.calls,
            stop: stop(end.stop),
            thought: (!self.thought.is_empty()).then_some(self.thought),
            usage: usage(end.usage),
            served,
        }
    }
}

/// The reason a reply ended.
pub fn stop(stop: sp::StopReason) -> pi::StopReason {
    match stop {
        sp::StopReason::EndTurn => pi::StopReason::EndTurn,
        sp::StopReason::ToolUse => pi::StopReason::ToolUse,
        sp::StopReason::MaxTokens => pi::StopReason::MaxTokens,
        sp::StopReason::StopSequence => pi::StopReason::StopSequence,
        sp::StopReason::ContentFilter => pi::StopReason::ContentFilter,
    }
}

/// The reason a reply ended, the other way (a replay of a history that carries one).
pub fn stop_back(stop: pi::StopReason) -> sp::StopReason {
    match stop {
        pi::StopReason::EndTurn => sp::StopReason::EndTurn,
        pi::StopReason::ToolUse => sp::StopReason::ToolUse,
        pi::StopReason::MaxTokens => sp::StopReason::MaxTokens,
        pi::StopReason::StopSequence => sp::StopReason::StopSequence,
        pi::StopReason::ContentFilter => sp::StopReason::ContentFilter,
    }
}

/// What a turn used, in the units the wire has.
pub fn usage(usage: sp::TurnUsage) -> pi::TokenUsage {
    pi::TokenUsage {
        input: Tokens(usage.input.0),
        output: Tokens(usage.output.0),
        cached: Tokens(usage.cached.0),
    }
}

/// The vectors of an embedding reply.
pub fn vectors(vectors: Vec<sp::EmbedVector>) -> Vec<pi::EmbedVector> {
    vectors
        .into_iter()
        .map(|vector| pi::EmbedVector(vector.0))
        .collect()
}

/// The width of a vector, for a reply that must match what was asked.
pub fn width(vector: &pi::EmbedVector) -> Dims {
    Dims(u32::try_from(vector.0.len()).unwrap_or(u32::MAX))
}

/// What the client is told when the engine fails. A gateway 5xx is a transient reach failure
/// (ARCHITECTURE section 7); a timeout has no variant of its own and is the same; a request the
/// engine rejected is told as a refusal (the model could not be asked), which the app cannot
/// fix by retrying.
pub fn model_error(error: &sp::ProviderError) -> pi::ModelError {
    match error {
        sp::ProviderError::Unreachable
        | sp::ProviderError::Timeout
        | sp::ProviderError::Server(_) => pi::ModelError::Unreachable,
        sp::ProviderError::NotReady => pi::ModelError::NotReady,
        sp::ProviderError::RateLimited(seconds) => pi::ModelError::RateLimited(seconds.0),
        sp::ProviderError::Unauthorized => pi::ModelError::Unauthorized,
        sp::ProviderError::ContextOverflow { .. } => pi::ModelError::ContextOverflow,
        sp::ProviderError::BadRequest(_) | sp::ProviderError::Refused(_) => pi::ModelError::Refused,
        sp::ProviderError::Unreadable(_) => pi::ModelError::Unreadable,
    }
}

#[cfg(test)]
mod tests;
