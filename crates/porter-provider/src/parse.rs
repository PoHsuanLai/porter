//! Reading provider files: text in, a checked [`ProviderSpec`] out. The caller reads the files
//! (accountd, from the system and user provider directories).

use crate::error::ProviderFileError;
use crate::spec::ProviderSpec;
use porter_core::{AuthKind, Capability, CapabilityKind, Family};

/// The provider declared by one file's text, or why it is refused.
pub fn parse_provider(text: &str) -> Result<ProviderSpec, ProviderFileError> {
    let spec: ProviderSpec =
        toml::from_str(text).map_err(|e| ProviderFileError::Syntax(e.to_string()))?;
    check(spec)
}

/// The rules the types alone do not hold.
fn check(spec: ProviderSpec) -> Result<ProviderSpec, ProviderFileError> {
    if spec.auth.needs_issuer() != spec.auth.issuer.is_some() {
        return Err(ProviderFileError::IssuerMismatch(spec.id));
    }
    if spec.capabilities.is_empty() {
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
    Ok(spec)
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
