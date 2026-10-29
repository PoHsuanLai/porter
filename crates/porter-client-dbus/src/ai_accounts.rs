//! accountd's `Broker` over the bus: the AI broker's questions.

use crate::Bus;
use crate::failure::{ai_accounts_error, link_error};
use futures_util::StreamExt;
use porter_capability::Claim;
use porter_client::{AiAccountsError, AiAccountsLink, Changes, LinkError, Malformed};
use porter_core::consent::Usage;
use porter_core::{AccountId, AccountState, AiAccountRow, AppId, DataClass, GrantId};
use porter_core::{LlmVerdict, OwnServerRow, ProviderId, SecretText};
use porter_dbus::accounts::{BrokerProxy, ai_account_from_dbus};
use porter_dbus::accounts::{own_server_from_dbus, verdict_from_dbus};
use porter_ids::word::word as word_of;
use std::io::Read;
use zeroize::Zeroizing;

/// The most a key may hold, as the client reads one.
const KEY_LIMIT: u64 = 16 * 1024;

fn malformed(what: Malformed) -> AiAccountsError {
    AiAccountsError::Link(LinkError::Malformed(what))
}

impl Bus {
    async fn broker(&self) -> Result<BrokerProxy<'static>, AiAccountsError> {
        self.accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| AiAccountsError::Link(link_error(&e)))
    }
}

/// A key read from the descriptor it arrived on.
fn read_key(fd: porter_dbus::zvariant::OwnedFd) -> Result<SecretText, AiAccountsError> {
    let owned: std::os::fd::OwnedFd = fd.into();
    let mut text = Zeroizing::new(String::new());
    std::fs::File::from(owned)
        .take(KEY_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|_| malformed(Malformed::Descriptor))?;
    if u64::try_from(text.len()).map_or(true, |len| len > KEY_LIMIT) {
        return Err(malformed(Malformed::Descriptor));
    }
    Ok(SecretText::new(text.as_str()))
}

impl AiAccountsLink for Bus {
    async fn verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
    ) -> Result<Vec<LlmVerdict>, AiAccountsError> {
        let who = (
            app.name.as_str().to_owned(),
            word_of(&app.isolation).to_owned(),
        );
        self.broker()
            .await?
            .verdicts(&who, word_of(&class), word_of(&usage))
            .await
            .map_err(|e| ai_accounts_error(&e))?
            .into_iter()
            .map(|row| verdict_from_dbus(row).map_err(|_| malformed(Malformed::Row)))
            .collect()
    }

    async fn key(&self, grant: &GrantId) -> Result<SecretText, AiAccountsError> {
        read_key(
            self.broker()
                .await?
                .read_key(grant.as_str())
                .await
                .map_err(|e| ai_accounts_error(&e))?,
        )
    }

    async fn accounts(&self) -> Result<Vec<AiAccountRow>, AiAccountsError> {
        self.broker()
            .await?
            .accounts()
            .await
            .map_err(|e| ai_accounts_error(&e))?
            .into_iter()
            .map(|row| ai_account_from_dbus(row).map_err(|_| malformed(Malformed::Row)))
            .collect()
    }

    async fn servers(&self) -> Result<Vec<OwnServerRow>, AiAccountsError> {
        self.broker()
            .await?
            .servers()
            .await
            .map_err(|e| ai_accounts_error(&e))?
            .into_iter()
            .map(|row| own_server_from_dbus(row).map_err(|_| malformed(Malformed::Row)))
            .collect()
    }

    async fn server_key(&self, account: &AccountId) -> Result<SecretText, AiAccountsError> {
        read_key(
            self.broker()
                .await?
                .read_server_key(account.as_str())
                .await
                .map_err(|e| ai_accounts_error(&e))?,
        )
    }

    async fn report_local(
        &self,
        provider: &ProviderId,
        claims: &[Claim],
        state: AccountState,
    ) -> Result<AccountId, AiAccountsError> {
        let claims =
            serde_json::to_string(claims).map_err(|_| AiAccountsError::Link(LinkError::Invalid))?;
        let id = self
            .broker()
            .await?
            .report_local(provider.as_str(), &claims, word_of(&state))
            .await
            .map_err(|e| ai_accounts_error(&e))?;
        AccountId::parse(&id).map_err(|_| malformed(Malformed::Id))
    }

    async fn changes(&self) -> Result<Changes<()>, AiAccountsError> {
        let stream = self
            .broker()
            .await?
            .receive_changed()
            .await
            .map_err(|e| ai_accounts_error(&e))?;
        Ok(Changes::of(stream.map(|_| ())))
    }
}
