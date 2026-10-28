//! What the AI accounts say to the daemon that sends a key to an AI company: the permits an app
//! holds on each account that serves language models, and the accounts that are servers of the
//! person's own. Plain data; no secret is in either.

use crate::consent::Verdict;
use crate::{AccountId, AccountState, ServiceEndpoint};
use serde::{Deserialize, Serialize};

/// The provider file whose accounts are servers of the person's own: an OpenAI-compatible
/// gateway, or a llama.cpp or vLLM on their network.
pub const OWN_SERVER_PROVIDER: &str = "openai-compatible";

/// What the permits say for an app on one account that could serve a language need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LlmVerdict {
    /// The account.
    pub account: AccountId,
    /// The provider file the account was made from.
    pub provider: String,
    /// What a person reads for the account.
    pub label: String,
    /// What a person reads for its provider, when the daemon knows.
    pub provider_label: Option<String>,
    /// Whether it works now; an account that must be signed in again is not a place that can
    /// serve.
    pub state: AccountState,
    /// The app's permit on it.
    pub verdict: Verdict,
}

impl LlmVerdict {
    /// The verdict `verdict` for `account`.
    pub fn new(
        account: AccountId,
        provider: String,
        label: String,
        provider_label: Option<String>,
        state: AccountState,
        verdict: Verdict,
    ) -> Self {
        Self {
            account,
            provider,
            label,
            provider_label,
            state,
            verdict,
        }
    }
}

/// An account that is a server of the person's own, and where it speaks the chat protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OwnServerRow {
    /// The account.
    pub account: AccountId,
    /// What the person calls it.
    pub label: String,
    /// Its chat-completions endpoint.
    pub endpoint: ServiceEndpoint,
}

impl OwnServerRow {
    /// The server `account` keeps at `endpoint`.
    pub fn new(account: AccountId, label: String, endpoint: ServiceEndpoint) -> Self {
        Self {
            account,
            label,
            endpoint,
        }
    }
}
