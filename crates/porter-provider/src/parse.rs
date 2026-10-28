//! Reading provider files: text in, a checked [`ProviderSpec`] out. The caller reads the files
//! (accountd, from the system and user provider directories).

use crate::error::ProviderFileError;
use crate::spec::ProviderSpec;
use porter_capability::{Capability, CapabilityKind, LlmWire};
use porter_core::{AuthKind, Family};
use porter_ids::word::{from_word, word};

/// The provider declared by one file's text, or why it is refused.
pub fn parse_provider(text: &str) -> Result<ProviderSpec, ProviderFileError> {
    let syntax = |e: &dyn std::fmt::Display| ProviderFileError::Syntax(e.to_string());
    let mut file: toml::Table = toml::from_str(text).map_err(|e| syntax(&e))?;
    if let Some(toml::Value::Array(rows)) = file.get_mut("capability") {
        rows.iter_mut().for_each(nest_capability);
    }
    let spec: ProviderSpec = toml::Value::Table(file)
        .try_into()
        .map_err(|e| syntax(&e))?;
    check(spec)
}

/// The keys of a capability row that are the row's own; every other key of the row is a field of
/// its capability.
const ROW_KEYS: [&str; 6] = [
    "family",
    "endpoint",
    "linked_origins",
    "auth_origins",
    "ask",
    "kind",
];

/// A capability row is written flat: `kind = "mail"` with the capability's fields beside it
/// (`access = "read_write"`, `send = "present"`). The types hold them in the bus form, the fields
/// under `v`, so this moves them there. A row that already has a `v` table (the old file shape,
/// which a person's own provider file may still be in) is left as it is.
fn nest_capability(row: &mut toml::Value) {
    let Some(row) = row.as_table_mut() else {
        return;
    };
    if row.contains_key("v") {
        return;
    }
    let mut fields: toml::Table = row
        .keys()
        .filter(|key| !ROW_KEYS.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|key| row.remove(&key).map(|value| (key, value)))
        .collect();
    // A language-model row says its request format once, as its family: `wire` is left out unless
    // it differs, which `check` refuses.
    let llm = row.get("kind").and_then(toml::Value::as_str) == Some("llm");
    if llm
        && !fields.contains_key("wire")
        && let Some(family) = row.get("family")
    {
        fields.insert("wire".to_owned(), family.clone());
    }
    row.insert("v".to_owned(), toml::Value::Table(fields));
}

/// The rules the types alone do not hold.
fn check(spec: ProviderSpec) -> Result<ProviderSpec, ProviderFileError> {
    if spec.auth.needs_issuer() != spec.auth.issuer.is_some() {
        return Err(ProviderFileError::IssuerMismatch(spec.id));
    }
    // A program that holds its own sign-in (Tailscale) offers apps nothing through
    // `Accounts1`: its service is its own bus object, so its file may list no capability.
    if spec.capabilities.is_empty() && spec.auth.kind != AuthKind::OwnProgram {
        return Err(ProviderFileError::NoCapabilities(spec.id));
    }
    let has_ai_kind = spec
        .capabilities
        .iter()
        .any(|row| row.capability.kind().is_ai());
    if has_ai_kind != spec.ai.is_some() {
        return Err(ProviderFileError::AiSpecMismatch(spec.id));
    }
    check_agent_rows(&spec)?;
    check_wires(&spec)?;
    Ok(spec)
}

/// A language-model row served by a family that is itself a request format (`chat_completions`,
/// `messages`, ...) speaks that format.
fn check_wires(spec: &ProviderSpec) -> Result<(), ProviderFileError> {
    for row in &spec.capabilities {
        if let Capability::Llm(llm) = &row.capability
            && from_word::<LlmWire>(word(&row.family)).is_some_and(|wire| wire != llm.wire)
        {
            return Err(ProviderFileError::WireFamily(spec.id.clone()));
        }
    }
    Ok(())
}

/// The rules for agent rows: family `acp_agent` for exactly the agent kind, each program once,
/// and an `agent_login` provider (an agent that signs itself in, holding no credential) has
/// agent rows and nothing else, so nothing in its file could be presented as a secret's use.
fn check_agent_rows(spec: &ProviderSpec) -> Result<(), ProviderFileError> {
    let mut programs = Vec::new();
    for row in &spec.capabilities {
        let is_agent = row.capability.kind() == CapabilityKind::Agent;
        if is_agent != (row.family == Family::AcpAgent) {
            return Err(ProviderFileError::AgentRows(spec.id.clone()));
        }
        if let Capability::Agent(agent) = &row.capability {
            if programs.contains(&&agent.program) {
                return Err(ProviderFileError::AgentRows(spec.id.clone()));
            }
            programs.push(&agent.program);
        }
    }
    let only_agents = spec
        .capabilities
        .iter()
        .all(|row| row.capability.kind() == CapabilityKind::Agent);
    if (spec.auth.kind == AuthKind::AgentLogin) && !only_agents {
        return Err(ProviderFileError::AgentRows(spec.id.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
