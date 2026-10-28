//! Whether a model can make a valid tool call (the check Automatic runs before it gives a model a
//! request that offers tools). The check is one canned request, made of made-up text and nothing
//! of the person's, and a pure judgement of the reply; running it is the daemon's.
//!
//! The right answer to [`probe_request`] is one call to [`PROBE_TOOL`] whose arguments are an
//! object holding the one string the schema asks for.

use crate::ids::{JsonSchemaText, JsonText, ToolName};
use crate::reply::ChatReply;
use crate::request::{ChatMessage, ChatRequest, MessagePart, Role, ToolDecl};
use porter_capability::Tier;
use porter_core::DataClass;
use porter_core::consent::Usage;
use serde::{Deserialize, Serialize};

/// The one tool the check offers.
pub const PROBE_TOOL: &str = "note_word";

/// The argument the tool takes.
const PROBE_ARG: &str = "word";

/// What the check found for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProof {
    /// It made one valid call.
    Passed,
    /// It did not: no call, a call to another function, arguments the schema refuses, or a
    /// reply that could not be read. A real request that ended this way counts the same.
    Failed,
}

/// The canned request: one tool, and a user message that asks for exactly one call to it.
/// `None` only if the fixed tool name or schema stopped parsing, which a test pins.
pub fn probe_request() -> Option<ChatRequest> {
    let schema = format!(
        r#"{{"type":"object","properties":{{"{PROBE_ARG}":{{"type":"string"}}}},"required":["{PROBE_ARG}"],"additionalProperties":false}}"#
    );
    let tool = ToolDecl {
        name: ToolName::parse(PROBE_TOOL).ok()?,
        description: "Writes down one word.".to_owned(),
        params: JsonSchemaText(JsonText::parse(&schema).ok()?),
    };
    let ask = ChatMessage {
        role: Role::User,
        parts: vec![MessagePart::Text(format!(
            "Use the tool {PROBE_TOOL} to write down the word \"maple\"."
        ))],
    };
    Some(
        ChatRequest::new(
            vec![ask],
            Tier::Balanced,
            DataClass::Public,
            Usage::Background,
        )
        .with_tools(vec![tool]),
    )
}

/// What `reply` to [`probe_request`] shows: `Passed` for exactly one call to the tool whose
/// arguments are an object with the one string and nothing else.
pub fn judge(reply: &ChatReply) -> ToolProof {
    let [call] = reply.tool_calls.as_slice() else {
        return ToolProof::Failed;
    };
    if call.name.as_str() != PROBE_TOOL {
        return ToolProof::Failed;
    }
    let args: Result<serde_json::Map<String, serde_json::Value>, _> =
        serde_json::from_str(call.args.as_str());
    match args {
        Ok(map) if map.len() == 1 && map.get(PROBE_ARG).is_some_and(|word| word.is_string()) => {
            ToolProof::Passed
        }
        _ => ToolProof::Failed,
    }
}

#[cfg(test)]
mod tests;
