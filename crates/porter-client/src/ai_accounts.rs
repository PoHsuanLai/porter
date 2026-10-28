//! What the AI broker (inferd) asks of the daemon that keeps the AI keys
//! (`org.quire.Accounts1.Broker`): which accounts an app holds a permit on, and the key behind a
//! permit. inferd is the one daemon that sends a key to an AI company, so the calls are its alone:
//! any other caller is refused. A key arrives on a sealed memfd, once, and is read into a
//! [`SecretText`], which shows nothing of itself.
//!
//! ```ignore
//! let accounts = AiAccounts::connect(&connection).await?;
//! let mut changes = accounts.changes().await?;                 // subscribe first
//! for row in accounts.verdicts(&app, DataClass::Prompt, Usage::Interactive).await? {
//!     if let Verdict::Granted { grant, .. } = row.verdict {
//!         let key = accounts.key(&grant).await?;               // for one request, then dropped
//!     }
//! }
//! ```

use crate::call_failure::{CallFailure, bus_failure_from};
use porter_capability::Claim;
use porter_core::consent::Usage;
use porter_core::{AccountId, AccountState, AiAccountRow, AppId, CoreError, DataClass, GrantId};
use porter_core::{LlmVerdict, OwnServerRow, ProviderId, SecretText};
use porter_dbus::accounts::refusal_of;
use porter_dbus::accounts::{BrokerChanged, BrokerChangedStream, BrokerProxy};
use porter_dbus::accounts::{ai_account_from_dbus, own_server_from_dbus, verdict_from_dbus};
use porter_dbus::{BusConnection, BusError, BusStream};
use porter_ids::IdError;
use porter_ids::word::word as word_of;
use porter_wire::Refusal;
use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll};

/// The most a key may hold, as the client reads one.
const KEY_LIMIT: u64 = 16 * 1024;

/// Why a call to the Broker failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AiAccountsError {
    /// accountd refused for a reason an app also meets: `UnknownGrant` (no such permit, or a
    /// once permit already spent), `AudienceNotGranted`, `Denied` (the person turned the account
    /// off), `NeedsReauth` (the key is gone or must be signed in again) or `Unavailable`.
    #[error("refused: {0:?}")]
    Refused(Refusal),
    /// The call itself failed: accountd is not there, did not know the caller as the AI broker,
    /// refused an argument, or sent something that is not porter's protocol.
    #[error(transparent)]
    Bus(CallFailure),
}

impl From<BusError> for AiAccountsError {
    fn from(error: BusError) -> Self {
        match refusal_of(&error) {
            Some(refusal) => AiAccountsError::Refused(refusal),
            None => AiAccountsError::Bus(error.into()),
        }
    }
}

bus_failure_from!(AiAccountsError: CallFailure, CoreError, IdError);

/// The AI accounts of accountd, as the AI broker asks them.
#[derive(Debug, Clone)]
pub struct AiAccounts {
    proxy: BrokerProxy<'static>,
}

impl AiAccounts {
    /// The Broker over `connection`, which accountd knows as the AI broker.
    pub async fn connect(connection: &BusConnection) -> Result<Self, AiAccountsError> {
        Self::connect_at(connection, &crate::Destination::WellKnown).await
    }

    /// The same, sent to `to`: the owner of accountd's name the caller checked.
    pub async fn connect_at(
        connection: &BusConnection,
        to: &crate::Destination,
    ) -> Result<Self, AiAccountsError> {
        Ok(Self {
            proxy: to.proxy(connection).await?,
        })
    }

    /// What the permits say for `app` on each account that serves language models, for data of
    /// `class` used as `usage` says.
    pub async fn verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
    ) -> Result<Vec<LlmVerdict>, AiAccountsError> {
        let who = (
            app.name.as_str().to_owned(),
            word_of(&app.isolation).to_owned(),
        );
        self.proxy
            .verdicts(&who, word_of(&class), word_of(&usage))
            .await?
            .into_iter()
            .map(|row| Ok(verdict_from_dbus(row)?))
            .collect()
    }

    /// The key behind the allow permit `grant` of the language-model kind, read once; an
    /// allow-once permit is spent by a read that succeeded. Hold it for one request and drop it.
    pub async fn key(&self, grant: &GrantId) -> Result<SecretText, AiAccountsError> {
        read_key(self.proxy.read_key(grant.as_str()).await?)
    }

    /// The AI accounts (the services, the programs on this computer and the assistants), for the
    /// rows the broker adds to them.
    pub async fn accounts(&self) -> Result<Vec<AiAccountRow>, AiAccountsError> {
        self.proxy
            .accounts()
            .await?
            .into_iter()
            .map(|row| Ok(ai_account_from_dbus(row)?))
            .collect()
    }

    /// The accounts that are servers of the person's own.
    pub async fn servers(&self) -> Result<Vec<OwnServerRow>, AiAccountsError> {
        self.proxy
            .servers()
            .await?
            .into_iter()
            .map(|row| Ok(own_server_from_dbus(row)?))
            .collect()
    }

    /// The key of the own server `account`, for the one request that asks it for its models. It
    /// needs no permit.
    pub async fn server_key(&self, account: &AccountId) -> Result<SecretText, AiAccountsError> {
        read_key(self.proxy.read_server_key(account.as_str()).await?)
    }

    /// Reports a probed local runtime as the account of `provider`, its models as `claims`, in
    /// `state` (`Ok` or `Offline`); the account's id.
    pub async fn report_local(
        &self,
        provider: &ProviderId,
        claims: &[Claim],
        state: AccountState,
    ) -> Result<AccountId, AiAccountsError> {
        let claims = serde_json::to_string(claims)
            .map_err(|why| CallFailure::Malformed(format!("claims: {why}")))?;
        let id = self
            .proxy
            .report_local(provider.as_str(), &claims, word_of(&state))
            .await?;
        Ok(AccountId::parse(&id)?)
    }

    /// The stream of "an AI account, a permit or a switch changed". Subscribe before the first
    /// look, so a change between the look and the subscription is not missed.
    pub async fn changes(&self) -> Result<Changes, AiAccountsError> {
        Ok(Changes {
            stream: self.proxy.receive_changed().await?,
        })
    }
}

/// A key read from the descriptor it arrived on.
fn read_key(fd: zbus::zvariant::OwnedFd) -> Result<SecretText, AiAccountsError> {
    let owned: std::os::fd::OwnedFd = fd.into();
    let mut text = zeroize::Zeroizing::new(String::new());
    std::fs::File::from(owned)
        .take(KEY_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|why| {
            CallFailure::Malformed(format!("the key could not be read: {:?}", why.kind()))
        })?;
    if u64::try_from(text.len()).map_or(true, |len| len > KEY_LIMIT) {
        return Err(CallFailure::Malformed("a key longer than any key".to_owned()).into());
    }
    Ok(SecretText::new(text.as_str()))
}

/// The stream of changes to the AI accounts; each item says only that something moved, and the
/// listener asks again.
#[derive(Debug)]
pub struct Changes {
    stream: BrokerChangedStream,
}

impl BusStream for Changes {
    type Item = ();

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<()>> {
        Pin::new(&mut self.stream)
            .poll_next(cx)
            .map(|signal| signal.map(|_: BrokerChanged| ()))
    }
}
