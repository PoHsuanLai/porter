//! Structured replies: validate and repair (ARCHITECTURE section 7, stoker interface ask 100).
//!
//! `ReplyShape::Json(schema)` and `ReplyShape::Choice` are read as stoker's `Shape` and run under
//! a `ShapedSession` around the turn, so the reply an app gets has passed `Shape::check`, or the
//! turn fails `Unparseable` (never a string that merely looks like it). The session picks how to
//! ask (`choose`: the engine's constraint, else a synthetic `final_result` tool, else the schema in
//! the prompt) and may repair (once by default), naming the field and never echoing the reply.
//!
//! What is not checked: a schema the shape vocabulary cannot say (a number, a `pattern`, an open
//! object: `Shape::from_json_schema` refuses it) is sent to the engine as the constraint, as
//! before, and the reply is passed on as the engine made it; so is a request that carries tools of
//! its own, because its turn may rightly end in a call of one of them and not in a reply of the
//! shape.
//!
//! The limits (what a schema may leave open, how deep it nests, how many repairs) come from the
//! daemon's configuration, `[ai.structured]` of `inferd.toml` (settings `ai.structured.*`, design/22
//! section 3.26), passed in as a [`Limits`]; the constants in `limits` are only the defaults.
//!
//! While the turns run only the thoughts stream: the text of a first attempt may be repaired, so
//! the reply is told once, when it has been checked.

use crate::bridge;
use crate::local::LocalModel;
use crate::tee::{Echo, Tee};
use model_extract::{ExtractFailure, Extracted, ShapedSession, ToolsPresent, choose};
use model_provider::{ChoiceText, Flow, JsonText, Provider, SchemaText, Shape, TurnRequest};
use porter_core::Tokens;
use porter_infer::{self as pi, InferEvent, ModelError};

pub mod limits;
pub use limits::{AiConfig, Limits, Resolved, StructuredConfig};

/// Whether a reply is a JSON value or one of the offered strings (which the app gets bare).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Json,
    Choice,
}

/// A session to run around the turn.
#[derive(Debug, Clone)]
pub struct Checked {
    session: ShapedSession,
    kind: Kind,
}

/// What the request's shape asks of its turn.
#[derive(Debug, Clone)]
pub enum Shaping {
    /// Send the turn as built; the reply is not checked.
    Unchecked,
    /// Check the reply, repair as often as the limits allow.
    Checked(Box<Checked>),
}

/// The shape of a request's reply as a value, when the vocabulary can say it.
fn read(shape: &pi::ReplyShape, limits: Limits) -> Option<(Shape, Kind)> {
    match shape {
        pi::ReplyShape::Text => None,
        pi::ReplyShape::Json(schema) => {
            let schema = SchemaText(JsonText::new(schema.as_str()).ok()?);
            Shape::from_json_schema(&schema, limits.schema)
                .ok()
                .map(|shape| (shape, Kind::Json))
        }
        pi::ReplyShape::Choice(choices) => (!choices.is_empty()).then(|| {
            let items = choices.iter().cloned().map(ChoiceText).collect();
            (Shape::Choice(items), Kind::Choice)
        }),
    }
}

/// How the request's reply is checked on this model.
pub fn shaping(model: &LocalModel, request: &pi::ChatRequest, limits: Limits) -> Shaping {
    let (Some(caps), Some(flavor)) = (model.caps(), model.flavor) else {
        return Shaping::Unchecked;
    };
    let Some((shape, kind)) = read(&request.shape, limits).filter(|_| request.tools.is_empty())
    else {
        return Shaping::Unchecked;
    };
    let mode = choose(
        caps,
        &shape,
        ToolsPresent::No,
        flavor.quirks().shape_with_tools,
    );
    Shaping::Checked(Box::new(Checked {
        session: ShapedSession::new(shape, mode, limits.repairs),
        kind,
    }))
}

/// A reply that passed its check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    /// The JSON as the model wrote it, or the chosen string.
    pub text: String,
    /// What it thought, when it did.
    pub thought: Option<String>,
    /// What every attempt used.
    pub usage: pi::TokenUsage,
}

fn add(a: Tokens, b: Tokens) -> Tokens {
    Tokens(a.0.saturating_add(b.0))
}

/// Why a shaped turn ended without a value, as the app is told.
fn failure(failure: ExtractFailure) -> ModelError {
    match failure {
        ExtractFailure::Refused => ModelError::Refused,
        ExtractFailure::Unparseable | ExtractFailure::Truncated | ExtractFailure::OverBudget => {
            ModelError::Unparseable
        }
    }
}

/// Runs the turn (and the repair, when there is one) until a reply passes its check. `forward`
/// takes the thoughts as they stream (a `Stop` from it ends the turn); `touch` is called after
/// each attempt, to tell the supervisor the engine is in use.
pub async fn run<P: Provider>(
    provider: &P,
    checked: Checked,
    base: &TurnRequest,
    forward: &mut (impl FnMut(InferEvent) -> Flow + Send),
    touch: impl Fn(),
) -> Result<Validated, ModelError> {
    let Checked { mut session, kind } = checked;
    let mut request = session.request(base);
    let mut usage = pi::TokenUsage {
        input: Tokens(0),
        output: Tokens(0),
        cached: Tokens(0),
    };
    let mut thought = String::new();
    loop {
        let mut tee = Tee::new(Echo::Thoughts, forward);
        let end = provider
            .turn(&request, &mut tee)
            .await
            .map_err(|error| bridge::model_error(&error))?;
        touch();
        if tee.flow() == Flow::Stop {
            return Err(ModelError::Unreachable);
        }
        let said = tee.finish(end);
        let used = bridge::usage(said.end.usage);
        usage = pi::TokenUsage {
            input: add(usage.input, used.input),
            output: add(usage.output, used.output),
            cached: add(usage.cached, used.cached),
        };
        thought.push_str(&said.thought);
        match session.absorb(base, &said.end, &said.text, &said.calls) {
            Extracted::Done(json) => {
                let text = match kind {
                    Kind::Json => json.as_str().to_owned(),
                    Kind::Choice => serde_json::from_str::<String>(json.as_str())
                        .unwrap_or_else(|_| json.as_str().to_owned()),
                };
                return Ok(Validated {
                    text,
                    thought: (!thought.trim().is_empty()).then_some(thought),
                    usage,
                });
            }
            Extracted::Repair(next) => request = *next,
            Extracted::Failed(why) => return Err(failure(why)),
        }
    }
}

#[cfg(test)]
mod tests;
