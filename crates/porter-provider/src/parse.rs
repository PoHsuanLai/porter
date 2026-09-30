//! Reading provider files: text in, a checked [`ProviderSpec`] out. The caller reads the files
//! (accountd, from the system and user provider directories).

use crate::error::ProviderFileError;
use crate::spec::ProviderSpec;

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
    Ok(spec)
}

#[cfg(test)]
mod tests;
