//! What a hosted model needs of the AI accounts: which accounts an app holds a permit on
//! ([`Accountd::verdicts`]) and, for the one it will call, the account's API key
//! ([`Accountd::key`]).
//!
//! inferd holds the keys itself (`keys::LocalKeys` implements this seam over its own store), so a
//! key is read in this process, into a [`SecretText`] (whose `Debug` shows nothing) that the
//! caller drops after building one request. The seam stays so the cloud turns are tested over a
//! scripted fake. The module keeps its old name; it is the "accounts" seam.
//!
//! Two more questions are about the servers of the person's own (`own`, accounts of the provider
//! `openai-compatible`): which accounts they are and where ([`Accountd::servers`]), and the key
//! of one to ask it for its models ([`Accountd::server_key`]), which no app asked for and so
//! needs no permit. A seam that has no such account answers none.

use super::own::OwnServer;
use porter_capability::LlmFeature;
use porter_core::consent::{Usage, Verdict};
use porter_core::{AccountId, GrantId};
use porter_core::{AccountState, AppId, DataClass, SecretText};
use porter_ids::Tokens;
use porter_match::LlmNeed;
use porter_match::Need;
use porter_wire::Refusal;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

/// A boxed future, so the seam can be a trait object.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a language need is, for the verdicts: chat, any context.
pub fn llm_need() -> Need {
    Need::Llm(LlmNeed::new([LlmFeature::Chat].into(), Tokens(0)))
}

/// What the permits say for an app on one account that could serve a need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountVerdict {
    /// The account.
    pub account: AccountId,
    /// The provider file the account was made from, when known; otherwise the account id's own
    /// stem names it.
    pub provider: Option<String>,
    /// What a person reads for the account.
    pub label: Option<String>,
    /// What a person reads for its provider.
    pub provider_label: Option<String>,
    /// Whether it works now; an account that must be signed in again is not a place that can
    /// serve.
    pub state: Option<AccountState>,
    /// The app's permit on it.
    pub verdict: Verdict,
}

/// Why the key store did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountdFault {
    /// The store could not be reached: ask again at the next look.
    Unreachable,
    /// The store refused this permit, or the account needs signing in again.
    Refused,
    /// The key was not readable.
    Unreadable,
}

impl From<Refusal> for AccountdFault {
    /// What inferd does about it: a refusal is final, an unavailable store is "ask again".
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Unavailable => AccountdFault::Unreachable,
            _ => AccountdFault::Refused,
        }
    }
}

/// The AI accounts, as inferd uses them.
pub trait Accountd: Debug + Send + Sync + 'static {
    /// The permits `app` holds, for data of `class` used as `usage`, on every account that serves
    /// a language need.
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>>;

    /// The API key behind `grant`.
    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>>;

    /// The accounts that are servers of the person's own, and where each is.
    fn servers(&self) -> Boxed<'_, Vec<OwnServer>> {
        Box::pin(async { Vec::new() })
    }

    /// The key of the server of `account`, for the one request that asks it for its models. An
    /// empty key is a server that needs none.
    fn server_key<'a>(
        &'a self,
        _account: &'a AccountId,
    ) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async { Err(AccountdFault::Refused) })
    }
}

#[cfg(test)]
mod tests;
