//! What both protocol readers share: the parsed request, the words of a refusal to map a feature,
//! the stop and usage vocabularies, and the event-stream writer both renderers implement.

use porter_core::{Permille, Tokens};
use porter_infer::{
    ChatReply, ChatRequest, Effort, Knob, Reasoning, Sampling, StopReason, TokenUsage, ToolCallPart,
};
use serde_json::Value;

/// A request read from an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The model id the agent named.
    pub model: String,
    /// Whether it asked for a stream.
    pub stream: bool,
    /// Whether a stream is to end with a usage chunk (OpenAI's `stream_options.include_usage`).
    pub include_usage: bool,
    /// The request, as inferd's own chat request.
    pub chat: ChatRequest,
    /// For OpenAI's `logprobs` on a `Choice`: how many of the options' log-probabilities the
    /// reply lists beside the chosen one (`top_logprobs`, 0 to 20). `None` when the request did
    /// not ask, or is not a `Choice`: the reply then has no `logprobs`.
    pub top_logprobs: Option<u32>,
}

/// A request that cannot be expressed to the model, and what in it could not be. The text names
/// the feature, never quotes the content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmapped(pub String);

impl Unmapped {
    /// A refusal naming `what`.
    pub fn of(what: impl Into<String>) -> Self {
        Self(what.into())
    }
}

/// The text of a JSON string field.
pub fn text_of<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

/// A temperature given as a decimal, in thousandths.
fn permille(value: f64) -> Permille {
    Permille((value.max(0.0) * 1000.0).round().min(f64::from(u32::MAX)) as u32)
}

/// The sampling a request names: `Off` when it names none of temperature, `top_p` or `top_k`.
/// A temperature left out takes `default_temperature` (a protocol's own default).
pub fn sampling_of(
    temperature: Option<f64>,
    top_p: Option<f64>,
    top_k: Option<u64>,
    default_temperature: f64,
) -> Knob<Sampling> {
    if temperature.is_none() && top_p.is_none() && top_k.is_none() {
        return Knob::Off;
    }
    Knob::Set(Sampling {
        temperature: permille(temperature.unwrap_or(default_temperature)),
        top_p: top_p.map_or(Knob::Off, |p| Knob::Set(permille(p))),
        top_k: top_k.map_or(Knob::Off, |k| {
            Knob::Set(porter_core::Count(u32::try_from(k).unwrap_or(u32::MAX)))
        }),
        min_p: Knob::Off,
        seed: Knob::Off,
    })
}

/// A reply limit given as a number.
pub fn limit_of(value: Option<u64>) -> Knob<Tokens> {
    value.map_or(Knob::Off, |n| {
        Knob::Set(Tokens(u32::try_from(n).unwrap_or(u32::MAX)))
    })
}

/// How hard to reason for a thinking budget in tokens.
pub fn effort_for_budget(budget: u64) -> Reasoning {
    Reasoning::On(match budget {
        0..=2_048 => Effort::Low,
        2_049..=12_288 => Effort::Medium,
        _ => Effort::High,
    })
}

/// Whether the reply is only reasoning: it says nothing and calls nothing.
pub fn only_thought(reply: &ChatReply) -> Option<(StopReason, u32)> {
    let thought = reply.thought.as_deref().filter(|t| !t.is_empty())?;
    (reply.text.is_empty() && reply.tool_calls.is_empty())
        .then(|| (reply.stop, u32::try_from(thought.len()).unwrap_or(u32::MAX)))
}

/// The tokens of a reply in the units both protocols count: uncached input, output, cached.
pub fn counted(usage: TokenUsage) -> (u32, u32, u32) {
    let cached = usage.cached.0.min(usage.input.0);
    (usage.input.0 - cached, usage.output.0, cached)
}

/// A tool call's arguments as JSON (the text was checked JSON where it entered).
pub fn args_value(call: &ToolCallPart) -> Value {
    serde_json::from_str(call.args.as_str()).unwrap_or(Value::Null)
}

/// What a renderer writes as a stream: the text of each piece, in order.
pub trait SseOut: Send {
    /// The head of the reply, before its first piece.
    fn begin(&mut self) -> String;
    /// Reasoning so far.
    fn thought(&mut self, text: &str) -> String;
    /// Reply text so far.
    fn text(&mut self, text: &str) -> String;
    /// A finished function call.
    fn tool(&mut self, call: &ToolCallPart) -> String;
    /// The end of a reply that finished.
    fn finish(&mut self, reply: &ChatReply) -> String;
    /// The end of a stream that failed after it began.
    fn error(&mut self, failure: &super::fail::Failure) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_infer::ServedBy;

    fn reply(text: &str, thought: Option<&str>, calls: usize) -> ChatReply {
        let made = ChatReply::new(
            text.into(),
            StopReason::EndTurn,
            TokenUsage {
                input: Tokens(10),
                output: Tokens(3),
                cached: Tokens(4),
            },
            ServedBy {
                account: porter_core::AccountId::parse("local").expect("id"),
                model: porter_core::ModelId::parse("m").expect("id"),
                locality: porter_core::Locality::OnDevice,
            },
        )
        .with_tool_calls(
            (0..calls)
                .map(|n| ToolCallPart {
                    id: porter_infer::ToolCallId(format!("c{n}")),
                    name: porter_infer::ToolName::parse("t").expect("name"),
                    args: porter_infer::JsonText::parse("{}").expect("json"),
                })
                .collect(),
        );
        match thought {
            Some(t) => made.with_thought(t.to_owned()),
            None => made,
        }
    }

    #[test]
    fn a_reply_of_only_reasoning_is_told_by_length() {
        let cases = [
            (reply("", Some("hmm"), 0), Some((StopReason::EndTurn, 3))),
            (reply("", Some("hmm"), 1), None),
            (reply("hi", Some("hmm"), 0), None),
            (reply("", None, 0), None),
            (reply("", Some(""), 0), None),
        ];
        for (reply, want) in cases {
            assert_eq!(only_thought(&reply), want);
        }
    }

    #[test]
    fn usage_is_counted_with_the_cached_share_apart() {
        assert_eq!(counted(reply("", None, 0).usage), (6, 3, 4));
    }

    #[test]
    fn sampling_is_left_to_the_model_unless_the_request_names_one() {
        assert_eq!(sampling_of(None, None, None, 1.0), Knob::Off);
        let Knob::Set(sampling) = sampling_of(Some(0.2), Some(0.9), Some(40), 1.0) else {
            panic!("a sampling");
        };
        assert_eq!(sampling.temperature, Permille(200));
        assert_eq!(sampling.top_p, Knob::Set(Permille(900)));
        let Knob::Set(sampling) = sampling_of(None, Some(0.5), None, 1.0) else {
            panic!("a sampling");
        };
        assert_eq!(sampling.temperature, Permille(1000));
    }

    #[test]
    fn a_thinking_budget_maps_to_an_effort() {
        assert_eq!(effort_for_budget(1024), Reasoning::On(Effort::Low));
        assert_eq!(effort_for_budget(8000), Reasoning::On(Effort::Medium));
        assert_eq!(effort_for_budget(32000), Reasoning::On(Effort::High));
    }
}
