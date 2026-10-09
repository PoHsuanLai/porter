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
    /// An agent program that signs itself in to its own plan (Claude Code, Gemini CLI, Codex):
    /// the account holds no credential of any kind, only whether the agent says it is signed
    /// in (design/31 R7, R8).
    AgentLogin,
    /// A program on this computer that holds the sign-in itself and answers porter when asked
    /// (Tailscale): the account holds no credential of any kind, and its state is what the
    /// program says it is. Signing in again is done in the program, never through porter.
    OwnProgram,
}

/// How a person signs an account in again, in the few ways a screen words differently ("in your
/// browser", "enter the password again"). Settings publishes it on each account's `sign_in` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInWay {
    /// In the browser: OAuth, or Nextcloud's Login Flow.
    Browser,
    /// By typing a password: the account's own, an app password, a bridge's.
    Password,
    /// By pasting a key: an API key or an access key pair.
    Key,
    /// Inside the agent program, which signs itself in.
    Agent,
    /// Outside porter: a cloud identity a command-line tool holds.
    Outside,
    /// Not at all: no credential (a probed runtime, an open endpoint).
    Nothing,
}

impl SignInWay {
    /// The stable slug, as the row's value says it.
    pub fn slug(self) -> &'static str {
        match self {
            SignInWay::Browser => "browser",
            SignInWay::Password => "password",
            SignInWay::Key => "key",
            SignInWay::Agent => "agent",
            SignInWay::Outside => "outside",
            SignInWay::Nothing => "nothing",
        }
    }
}

impl AuthKind {
    /// How an account of this kind is signed in again. No wildcard arm: a new kind is placed here.
    pub fn sign_in_way(self) -> SignInWay {
        match self {
            AuthKind::OAuthPkce
            | AuthKind::OAuthMintsKey
            | AuthKind::OAuthPlan
            | AuthKind::LoginFlowV2 => SignInWay::Browser,
            AuthKind::Password | AuthKind::AppPassword | AuthKind::LocalBridge => {
                SignInWay::Password
            }
            AuthKind::ApiKey | AuthKind::KeyPair => SignInWay::Key,
            AuthKind::AgentLogin => SignInWay::Agent,
            AuthKind::CloudIdentity | AuthKind::OwnProgram => SignInWay::Outside,
            AuthKind::None | AuthKind::LocalRuntime => SignInWay::Nothing,
        }
    }
}
