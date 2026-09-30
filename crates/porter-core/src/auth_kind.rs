//! How an account signs in (design/31 §3.1). A closed set: a new kind needs code.

use serde::{Deserialize, Serialize};

/// The sign-in method of a provider and of each account made from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// A remote endpoint that needs no credential (a vLLM on the local network).
    None,
    /// A runtime on this computer found by a probe (Ollama, llama.cpp, ComfyUI).
    LocalRuntime,
    /// The account password (generic IMAP/DAV servers).
    Password,
    /// A provider-issued app password (iCloud, Fastmail).
    AppPassword,
    /// Nextcloud's Login Flow v2, which mints an app password.
    LoginFlowV2,
    /// A local bridge's own password (Proton Bridge).
    LocalBridge,
    /// An access key pair (S3, B2).
    KeyPair,
    /// An API key the user pastes (Anthropic, OpenAI, Gemini).
    ApiKey,
    /// OAuth 2.0 with PKCE and a loopback redirect (Google, Microsoft, Dropbox).
    #[serde(rename = "oauth_pkce")]
    OAuthPkce,
    /// OAuth PKCE that mints a user-owned API key (OpenRouter, R10).
    #[serde(rename = "oauth_mints_key")]
    OAuthMintsKey,
    /// OAuth that spends a subscription plan's allowance (ChatGPT sign-in, R9; an option).
    #[serde(rename = "oauth_plan")]
    OAuthPlan,
    /// A cloud identity (Entra, Vertex ADC, an AWS profile).
    CloudIdentity,
}
