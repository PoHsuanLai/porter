//! The replay cassette: a file of (request match, reply) pairs, read from the path
//! `inferd.toml` names, matched against the chat requests inferd sends an engine.
//!
//! The file is stoker's `model-replay` shape: JSON Lines, a header line (its `CassetteHeader`:
//! version, engine stamp, model, context) and then one [`Entry`] per line. The header is
//! stoker's; the lines are inferd's own, because a wire cassette matches a request by its whole
//! body and a scripted conversation cannot (a planner's view carries ids the cassette's author
//! cannot know), so an entry says what a request must look like ([`When`]) instead. The reply of
//! an entry may be a recorded `WireReply` of stoker's.

use model_replay::{CassetteError, CassetteHeader, CassetteVersion, WireReply};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Whether a request carries tools (the planner asks with them; a structured reply never does).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tools {
    /// Either.
    #[default]
    Any,
    /// The request offers at least one tool.
    Present,
    /// The request offers none.
    Absent,
}

/// What a request must look like for an entry to answer it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct When {
    /// Tools offered or not.
    #[serde(default)]
    pub tools: Tools,
    /// Every one of these appears in the text of the request's messages.
    #[serde(default)]
    pub contains: Vec<String>,
    /// None of these does.
    #[serde(default)]
    pub lacks: Vec<String>,
}

/// One tool call a reply makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// The tool's name on the wire.
    pub name: String,
    /// Its arguments, as JSON.
    pub arguments: Value,
}

/// What an engine says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Reply {
    /// Words that end the turn.
    Text(String),
    /// Tool calls, in order.
    Calls(Vec<Call>),
    /// An HTTP error with this status.
    Fail(u16),
    /// A recorded exchange of a real engine, frame by frame.
    Wire(WireReply),
}

/// How many requests an entry answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Uses {
    /// One; later matching requests find the next entry.
    #[default]
    Once,
    /// Every matching request.
    Always,
}

/// One line of the file after the header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The request it answers.
    pub when: When,
    /// The answer.
    pub reply: Reply,
    /// How often.
    #[serde(default)]
    pub uses: Uses,
}

/// The parts of a request the match reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// Whether the request offers a tool.
    pub tools: bool,
    /// Every text of every message, joined by newlines.
    pub text: String,
}

impl Seen {
    /// What the body of a chat request shows.
    pub fn of(body: &Value) -> Self {
        let texts = body
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|message| texts_of(message.get("content")));
        Self {
            tools: body
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| !tools.is_empty()),
            text: texts.collect::<Vec<_>>().join("\n"),
        }
    }
}

fn texts_of(content: Option<&Value>) -> Vec<String> {
    match content {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

impl When {
    /// Whether a request of this look is the one this describes.
    pub fn admits(&self, seen: &Seen) -> bool {
        let tools = match self.tools {
            Tools::Any => true,
            Tools::Present => seen.tools,
            Tools::Absent => !seen.tools,
        };
        tools
            && self.contains.iter().all(|word| seen.text.contains(word))
            && !self.lacks.iter().any(|word| seen.text.contains(word))
    }
}

/// A cassette in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cassette {
    /// Stoker's header.
    pub header: CassetteHeader,
    /// The entries, in file order.
    pub entries: Vec<Entry>,
}

impl Cassette {
    /// The cassette in `text`: the header, then one entry per line. A blank line is skipped.
    pub fn parse(text: &str) -> Result<Self, CassetteError> {
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty());
        let (_, first) = lines.next().ok_or(CassetteError::Empty)?;
        let header: CassetteHeader =
            serde_json::from_str(first).map_err(|_| CassetteError::BadLine { line: 1 })?;
        if header.vocab != CassetteVersion::CURRENT {
            return Err(CassetteError::Version {
                found: header.vocab,
                want: CassetteVersion::CURRENT,
            });
        }
        let entries = lines
            .map(|(at, line)| {
                serde_json::from_str(line).map_err(|_| CassetteError::BadLine {
                    line: u32::try_from(at + 1).unwrap_or(u32::MAX),
                })
            })
            .collect::<Result<Vec<Entry>, _>>()?;
        Ok(Self { header, entries })
    }

    /// The first entry that admits `seen` and has not used itself up. `used` is the indexes of
    /// the `Once` entries answered so far.
    pub fn pick(&self, seen: &Seen, used: &[usize]) -> Option<usize> {
        self.entries.iter().enumerate().find_map(|(at, entry)| {
            let spent = entry.uses == Uses::Once && used.contains(&at);
            (!spent && entry.when.admits(seen)).then_some(at)
        })
    }
}

/// A header line for tests of this module tree.
#[cfg(test)]
pub(crate) const TEST_HEADER: &str = r#"{"vocab":1,"engine":{"kind":"replay","build":"test"},"model":"scripted","recorded":0,"context":{"loaded":4096,"trained":4096},"speech":null}"#;

#[cfg(test)]
mod tests;
