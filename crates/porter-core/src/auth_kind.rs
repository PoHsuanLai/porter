//! How an account signs in (design/31 §3.1). A closed set: a new kind needs code.

use crate::account::AccountState;
use crate::identities::IdentityKind;
use crate::secret::SecretPurpose;
use serde::{Deserialize, Serialize};

/// The sign-in method of a provider and of each account made from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
#[non_exhaustive]
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

/// What is true of every account of one [`AuthKind`]: the one table the code asks, in place of a
/// `match` on the kind in each place that needs one of these facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthFacts {
    /// How a person signs an account of this kind in again.
    pub way: SignInWay,
    /// The identity kind such an account is, or `None` when it is not an identity (a local
    /// runtime, a key, an agent's own login, Tailscale).
    pub identity: Option<IdentityKind>,
    /// The secret such an account presents to its provider, or `None` for kinds with none (an
    /// agent that signs itself in holds its login itself, and so does Tailscale).
    pub secret: Option<SecretPurpose>,
    /// The state a new account of this kind starts in: working, except an agent that signs itself
    /// in, which waits for the agent to say it is (`Peer.SetAgentState`).
    pub first_state: AccountState,
    /// Whether such an account is a runtime on this computer found by a probe.
    pub probed: bool,
}

impl AuthKind {
    /// The facts of this kind. No wildcard arm: a new kind is placed here.
    pub fn facts(self) -> AuthFacts {
        let base = AuthFacts {
            way: SignInWay::Nothing,
            identity: None,
            secret: None,
            first_state: AccountState::Ok,
            probed: false,
        };
        let password = AuthFacts {
            way: SignInWay::Password,
            secret: Some(SecretPurpose::Password),
            ..base
        };
        let browser = AuthFacts {
            way: SignInWay::Browser,
            ..base
        };
        match self {
            AuthKind::None => base,
            AuthKind::LocalRuntime => AuthFacts {
                probed: true,
                ..base
            },
            AuthKind::Password | AuthKind::AppPassword => AuthFacts {
                identity: Some(IdentityKind::Password),
                ..password
            },
            AuthKind::LocalBridge => password,
            AuthKind::LoginFlowV2 => AuthFacts {
                secret: Some(SecretPurpose::Password),
                ..browser
            },
            AuthKind::KeyPair => AuthFacts {
                way: SignInWay::Key,
                secret: Some(SecretPurpose::KeyPair),
                ..base
            },
            AuthKind::ApiKey => AuthFacts {
                way: SignInWay::Key,
                secret: Some(SecretPurpose::ApiKey),
                ..base
            },
            AuthKind::OAuthPkce => AuthFacts {
                identity: Some(IdentityKind::OAuth),
                secret: Some(SecretPurpose::OAuthRefresh),
                ..browser
            },
            AuthKind::OAuthMintsKey => AuthFacts {
                secret: Some(SecretPurpose::ApiKey),
                ..browser
            },
            AuthKind::OAuthPlan => AuthFacts {
                secret: Some(SecretPurpose::OAuthRefresh),
                ..browser
            },
            AuthKind::CloudIdentity => AuthFacts {
                way: SignInWay::Outside,
                secret: Some(SecretPurpose::OAuthRefresh),
                ..base
            },
            AuthKind::AgentLogin => AuthFacts {
                way: SignInWay::Agent,
                first_state: AccountState::NeedsLogin,
                ..base
            },
            AuthKind::OwnProgram => AuthFacts {
                way: SignInWay::Outside,
                ..base
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_has_its_facts() {
        use AuthKind as A;
        use SignInWay as W;
        let ok = AccountState::Ok;
        // kind, way, identity, secret, first state
        let table = [
            (A::None, W::Nothing, None, None, ok),
            (A::LocalRuntime, W::Nothing, None, None, ok),
            (
                A::Password,
                W::Password,
                Some(IdentityKind::Password),
                Some(SecretPurpose::Password),
                ok,
            ),
            (
                A::AppPassword,
                W::Password,
                Some(IdentityKind::Password),
                Some(SecretPurpose::Password),
                ok,
            ),
            (
                A::LoginFlowV2,
                W::Browser,
                None,
                Some(SecretPurpose::Password),
                ok,
            ),
            (
                A::LocalBridge,
                W::Password,
                None,
                Some(SecretPurpose::Password),
                ok,
            ),
            (A::KeyPair, W::Key, None, Some(SecretPurpose::KeyPair), ok),
            (A::ApiKey, W::Key, None, Some(SecretPurpose::ApiKey), ok),
            (
                A::OAuthPkce,
                W::Browser,
                Some(IdentityKind::OAuth),
                Some(SecretPurpose::OAuthRefresh),
                ok,
            ),
            (
                A::OAuthMintsKey,
                W::Browser,
                None,
                Some(SecretPurpose::ApiKey),
                ok,
            ),
            (
                A::OAuthPlan,
                W::Browser,
                None,
                Some(SecretPurpose::OAuthRefresh),
                ok,
            ),
            (
                A::CloudIdentity,
                W::Outside,
                None,
                Some(SecretPurpose::OAuthRefresh),
                ok,
            ),
            (
                A::AgentLogin,
                W::Agent,
                None,
                None,
                AccountState::NeedsLogin,
            ),
            (A::OwnProgram, W::Outside, None, None, ok),
        ];
        for (kind, way, identity, secret, first_state) in table {
            let facts = kind.facts();
            assert_eq!(facts.way, way, "{kind:?}");
            assert_eq!(facts.identity, identity, "{kind:?}");
            assert_eq!(facts.secret, secret, "{kind:?}");
            assert_eq!(facts.first_state, first_state, "{kind:?}");
            assert_eq!(facts.probed, kind == A::LocalRuntime, "{kind:?}");
        }
    }
}
