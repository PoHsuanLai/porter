//! What the AI broker (inferd) asks of the daemon that keeps the AI keys
//! (`org.quire.Accounts1.Broker`): which accounts an app holds a permit on, and the key behind a
//! permit. inferd is the one daemon that sends a key to an AI company, so the calls are its alone:
//! any other caller is refused. A key arrives on a sealed memfd, once, and is read into a
//! [`SecretText`], which shows nothing of itself.
//!
//! ```ignore
//! let accounts = porter.ai_accounts();
//! let mut changes = accounts.changes().await?;                 // subscribe first
//! for row in accounts.verdicts(&app, DataClass::Prompt, Usage::Interactive).await? {
//!     if let Verdict::Granted { grant, .. } = row.verdict {
//!         let key = accounts.key(&grant).await?;               // for one request, then dropped
//!     }
//! }
//! ```

use crate::changes::Changes;
use crate::error::LinkError;
use crate::link::AiAccountsLink;
use porter_capability::Claim;
use porter_core::consent::Usage;
use porter_core::{AccountId, AccountState, AiAccountRow, AppId, DataClass, GrantId};
use porter_core::{LlmVerdict, OwnServerRow, ProviderId, SecretText};
use porter_wire::Refusal;
use std::sync::Arc;

/// Why a call to the Broker failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AiAccountsError {
    /// accountd refused for a reason an app also meets: `UnknownGrant` (no such permit, or a
    /// once permit already spent), `AudienceNotGranted`, `Denied` (the person turned the account
    /// off), `NeedsReauth` (the key is gone or must be signed in again) or `Unavailable`.
    #[error("refused")]
    Refused(Refusal),
    /// The call did not complete: accountd is not there, did not know the caller as the AI broker,
    /// refused an argument, or sent something that is not porter's protocol.
    #[error(transparent)]
    Link(#[from] LinkError),
}

/// The AI accounts of accountd, as the AI broker asks them.
#[derive(Debug)]
pub struct AiAccounts<T> {
    pub(crate) link: Arc<T>,
}

impl<T> Clone for AiAccounts<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: AiAccountsLink> AiAccounts<T> {
    /// What the permits say for `app` on each account that serves language models, for data of
    /// `class` used as `usage` says.
    pub async fn verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
    ) -> Result<Vec<LlmVerdict>, AiAccountsError> {
        self.link.verdicts(app, class, usage).await
    }

    /// The key behind the allow permit `grant` of the language-model kind, read once; an
    /// allow-once permit is spent by a read that succeeded. Hold it for one request and drop it.
    pub async fn key(&self, grant: &GrantId) -> Result<SecretText, AiAccountsError> {
        self.link.key(grant).await
    }

    /// The AI accounts (the services, the programs on this computer and the assistants), for the
    /// rows the broker adds to them.
    pub async fn accounts(&self) -> Result<Vec<AiAccountRow>, AiAccountsError> {
        self.link.accounts().await
    }

    /// The accounts that are servers of the person's own.
    pub async fn servers(&self) -> Result<Vec<OwnServerRow>, AiAccountsError> {
        self.link.servers().await
    }

    /// The key of the own server `account`, for the one request that asks it for its models. It
    /// needs no permit.
    pub async fn server_key(&self, account: &AccountId) -> Result<SecretText, AiAccountsError> {
        self.link.server_key(account).await
    }

    /// Reports a probed local runtime as the account of `provider`, its models as `claims`, in
    /// `state` (`Ok` or `Offline`); the account's id.
    pub async fn report_local(
        &self,
        provider: &ProviderId,
        claims: &[Claim],
        state: AccountState,
    ) -> Result<AccountId, AiAccountsError> {
        self.link.report_local(provider, claims, state).await
    }

    /// "An AI account, a permit or a switch changed", from now on; each item says only that
    /// something moved, and the listener asks again. Subscribe before the first look, so a change
    /// between the look and the subscription is not missed.
    pub async fn changes(&self) -> Result<Changes<()>, AiAccountsError> {
        self.link.changes().await
    }
}
