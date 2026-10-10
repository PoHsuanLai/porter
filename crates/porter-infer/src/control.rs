//! The controls of a chat turn beyond its messages (design/31 §5.5): which tool the model may
//! call, how many at once, how long the reply may be, whether it reasons, how it samples, where
//! it stops. All typed: there is no untyped parameter bag, so a knob is a field here or it does
//! not exist. They mirror stoker's `TurnRequest` (stoker has no porter dependency; `inferd::bridge`
//! converts).

use crate::ids::{OpaqueText, SignatureText, ToolName};
use crate::scores::ScoreOptions;
use porter_core::{Count, Permille, Tokens};
use serde::{Deserialize, Serialize};

/// A setting that is either left to the model's catalog default or set to a value. Written out in
/// full (an `Option` would be a field an app can leave out unnoticed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Knob<T> {
    /// The model's own default (the catalog entry's, for local models).
    #[default]
    Off,
    /// This value.
    Set(T),
}

impl<T> Knob<T> {
    /// Whether the knob is left to the default.
    pub fn is_off(&self) -> bool {
        matches!(self, Knob::Off)
    }
}

/// A random seed. 32 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seed(pub u32);

/// How a model samples its next token. Thousandths: `Permille(700)` is a temperature of 0.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Sampling {
    /// Temperature.
    pub temperature: Permille,
    /// Nucleus cut-off.
    pub top_p: Knob<Permille>,
    /// Keep the k likeliest tokens.
    pub top_k: Knob<Count>,
    /// Drop tokens under this share of the likeliest.
    pub min_p: Knob<Permille>,
    /// For a repeatable reply.
    pub seed: Knob<Seed>,
}

/// Which function the model may call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolChoice {
    /// The model decides.
    Auto,
    /// None: reply in text.
    Never,
    /// It must call one.
    Required,
    /// It must call this one (an engine that cannot enforce that refuses the request).
    Named(ToolName),
}

/// Whether a turn may make several calls or only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolParallelism {
    /// One call, then the result comes back.
    One,
    /// Several, in order.
    Many,
}

/// Whether the model reasons before it replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Reasoning {
    /// Whatever the engine does.
    EngineDefault,
    /// No reasoning.
    Off,
    /// Reasoning, this hard.
    On(Effort),
}

/// How hard a model reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    /// Briefly.
    Low,
    /// Moderately.
    Medium,
    /// At length.
    High,
}

/// Everything a chat turn asks beyond its messages and shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChatControl {
    /// Which function it may call.
    pub tool_choice: ToolChoice,
    /// One call at a time or several.
    pub tool_calls: ToolParallelism,
    /// The longest reply this turn wants (a reviewer's one-token verdict); `Off` leaves it to the
    /// model's own limit.
    pub max_output: Knob<Tokens>,
    /// Whether it reasons.
    pub reasoning: Reasoning,
    /// How it samples; `Off` takes the model's catalog default, so an app that does not care
    /// writes no number.
    pub sampling: Knob<Sampling>,
    /// Strings that end the reply.
    pub stop: Vec<String>,
    /// For a `Choice` reply: also report how likely each option was (`ChatReply::scores`). `Off`
    /// asks for nothing and is not written on the wire, so a client that never heard of the knob
    /// and a daemon that has not either read each other's frames. Ignored for any other shape.
    #[serde(default, skip_serializing_if = "Knob::is_off")]
    pub scores: Knob<ScoreOptions>,
}

impl ChatControl {
    /// The plain controls: the model decides about functions, one call at a time, no output
    /// limit, whatever the engine does about reasoning, the model's own sampling, no stop
    /// strings, and no scores.
    pub fn new() -> Self {
        Self {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::One,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: Vec::new(),
            scores: Knob::Off,
        }
    }

    /// The same controls with this tool choice.
    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = tool_choice;
        self
    }

    /// The same controls with this tool parallelism.
    pub fn with_tool_calls(mut self, tool_calls: ToolParallelism) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// The same controls with this output limit.
    pub fn with_max_output(mut self, max_output: Knob<Tokens>) -> Self {
        self.max_output = max_output;
        self
    }

    /// The same controls with this reasoning.
    pub fn with_reasoning(mut self, reasoning: Reasoning) -> Self {
        self.reasoning = reasoning;
        self
    }

    /// The same controls with this sampling.
    pub fn with_sampling(mut self, sampling: Knob<Sampling>) -> Self {
        self.sampling = sampling;
        self
    }

    /// The same controls with these stop strings.
    pub fn with_stop(mut self, stop: Vec<String>) -> Self {
        self.stop = stop;
        self
    }

    /// The same controls asking for these option scores.
    pub fn with_scores(mut self, scores: Knob<ScoreOptions>) -> Self {
        self.scores = scores;
        self
    }
}

impl Default for ChatControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a reply ended. Without it a planner cannot tell a turn cut by `max_output` from a
/// finished one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopReason {
    /// The model finished.
    EndTurn,
    /// It called a function and waits for the result.
    ToolUse,
    /// It hit the output limit: the reply, and any call in it, may be cut.
    MaxTokens,
    /// It reached a stop string.
    StopSequence,
    /// The provider's content filter ended it.
    ContentFilter,
}

/// What a provider attached to a thought so that it can be handed back unchanged on the next
/// turn. Apps never construct one; inferd keeps it across tool turns. Local engines send none.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ThoughtSeal {
    /// None.
    None,
    /// The thought, signed by the provider.
    Signed(SignatureText),
    /// The provider withheld the text; this is the encrypted block.
    Redacted(OpaqueText),
}
