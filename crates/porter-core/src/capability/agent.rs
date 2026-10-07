//! External coding agents (design/31 R7, R8; agent-session ask P1): a program that talks ACP
//! and is run by the launcher, not by porter. An account that "runs" an agent is either the
//! agent's own login (`AuthKind::AgentLogin`, no credential held here) or an API-key account
//! whose provider file names the agent programs allowed to use its key. This module holds the
//! typed facts about a program: what it is called, how it takes a key and a base URL, and which
//! protocols it speaks to a model.

use super::terms::Offered;
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The name of an agent program (`claude-code`, `gemini-cli`, `codex`, or a generic ACP agent's
/// id): 1 to 48 bytes of lowercase ASCII letters, digits and `-`, starting with a letter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentProgram(String);

impl AgentProgram {
    /// The program written as `text`.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let mut bytes = text.bytes();
        let ok = text.len() <= 48
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "agent program",
                text: text.to_owned(),
            })
        }
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AgentProgram {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<AgentProgram> for String {
    fn from(program: AgentProgram) -> String {
        program.0
    }
}

impl std::fmt::Display for AgentProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of an environment variable (`ANTHROPIC_API_KEY`): 1 to 64 bytes of uppercase ASCII
/// letters, digits and `_`, not starting with a digit. Only the name; a value is never here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EnvName(String);

impl EnvName {
    /// The variable name written as `text`.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let mut bytes = text.bytes();
        let ok = text.len() <= 64
            && bytes
                .next()
                .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
            && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "environment variable name",
                text: text.to_owned(),
            })
        }
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EnvName {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<EnvName> for String {
    fn from(name: EnvName) -> String {
        name.0
    }
}

/// A request format an agent speaks to its model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProtocol {
    /// OpenAI-compatible Chat Completions or Responses.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    /// Anthropic Messages.
    AnthropicMessages,
    /// Gemini generateContent.
    GenerateContent,
}

/// An agent program an account can run (family `acp_agent`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentCap {
    /// The program.
    pub program: AgentProgram,
    /// The environment variable the program reads an API key from, or none for a program that
    /// takes no key that way (it signs in itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_env: Option<EnvName>,
    /// The environment variable that overrides the program's model endpoint, or none for a
    /// program that cannot be pointed elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_env: Option<EnvName>,
    /// The protocols it speaks to a model.
    pub protocols: BTreeSet<AgentProtocol>,
}

impl AgentCap {
    /// Whether the program can be pointed at another model endpoint.
    pub fn base_url(&self) -> Offered {
        match self.base_url_env {
            Some(_) => Offered::Present,
            None => Offered::Absent,
        }
    }
}
