//! Issuing a token: the checks of `IssueToken` and the mint behind them. For an OAuth credential
//! the refresh goes one at a time per account (see [`Flights`](crate::flight::Flights)): a
//! second caller waits for the first and uses the token the first issued.

use crate::audience::covers;
use crate::audit::{AuditEvent, AuditSink};
use crate::ctx::Ctx;
use crate::sheets::Sheets;
use crate::token::{provider_refusal, secret_purpose, secrets_refusal};
use porter_core::capability::StorageScope;
use porter_core::consent::GrantScope;
use porter_core::store::RegistryStore;
use porter_core::{
    Account, AccountState, AppId, Audience, CapabilityKind, Credential, IssuedToken, SecretKey,
    SecretPurpose,
};
use porter_core::{AccountId, GrantId};
use porter_ids::clock::Clock;
use porter_provider::{Presented, Provider, ProviderError, ProviderSession};
use porter_secrets::Secrets;
use porter_wire::AccountsReply;
use porter_wire::Refusal;

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    Ctx<P, S, U, K, R, A>
{
    /// A short-lived token for `audience` under a grant of a kind and reach (`grant`) from
    /// `account`'s session, as the caller receives it. For an OAuth credential the refresh goes
    /// one at a time per account: a second caller waits for the first, then uses the token the
    /// first issued (one call to the issuer serves both) or, when that is not good for this
    /// audience, kind and reach, refreshes from what the first stored. A credential the provider rotated is written back before the token is
    /// returned. Other credentials are not rotated, so they are not queued.
    pub(crate) async fn issue_for(
        &self,
        account: &Account,
        provider: &P,
        purpose: Option<SecretPurpose>,
        audience: &Audience,
        grant: (CapabilityKind, StorageScope),
    ) -> Result<IssuedToken, Refusal> {
        let Some(purpose) = purpose else {
            let (token, _) = self
                .mint(
                    account,
                    provider,
                    Presented::Anonymous,
                    None,
                    audience,
                    grant,
                )
                .await?;
            return Ok(token);
        };
        let key = SecretKey {
            account: account.id.clone(),
            purpose,
        };
        let first = self.secrets.get(&key).await.map_err(secrets_refusal)?;
        if !matches!(first, Credential::OAuth { .. }) {
            let (token, _) = self
                .mint(
                    account,
                    provider,
                    Presented::Credential(first),
                    Some(&key),
                    audience,
                    grant,
                )
                .await?;
            return Ok(token);
        }
        let flight = self.flights.of(&account.id);
        let seen = flight.refreshes();
        let _turn = flight.gate.lock().await;
        // Read again under the gate: the caller ahead of us may have rotated the credential.
        let stored = self.secrets.get(&key).await.map_err(secrets_refusal)?;
        // Only a refresh made while we waited is shared; otherwise this caller asks the issuer.
        if flight.refreshes() != seen
            && let Some(token) = flight.reusable(&stored, audience, grant, self.clock.now())
        {
            return Ok(token);
        }
        let (token, renewed) = self
            .mint(
                account,
                provider,
                Presented::Credential(stored.clone()),
                Some(&key),
                audience,
                grant,
            )
            .await?;
        let left = renewed.unwrap_or_else(|| stored.clone());
        flight.remember(&stored, left, audience, grant, token.clone());
        Ok(token)
    }

    /// Opens `account` with `presented`, asks the session for a token, and files a credential it
    /// rotated under `key` before returning the token. The credential filed is returned too.
    async fn mint(
        &self,
        account: &Account,
        provider: &P,
        presented: Presented,
        key: Option<&SecretKey>,
        audience: &Audience,
        (kind, reach): (CapabilityKind, StorageScope),
    ) -> Result<(IssuedToken, Option<Credential>), Refusal> {
        let session = match provider.open(&account.id, presented).await {
            Ok(session) => session,
            Err(error) => return Err(self.refused_refresh(&account.id, error).await),
        };
        // The grant's kind and reach, so the token reaches that kind's service alone, as far as
        // the grant does.
        let token = match session.access_token_for(audience, kind, reach).await {
            Ok(token) => token,
            Err(error) => return Err(self.refused_refresh(&account.id, error).await),
        };
        let renewed = session.renewed();
        if let (Some(renewed), Some(key)) = (&renewed, key) {
            self.secrets
                .put(key, renewed)
                .await
                .map_err(secrets_refusal)?;
        }
        Ok((token, renewed))
    }

    /// A refresh the provider refused (`Unauthorized`) leaves the account `NeedsReauth`, which
    /// the host announces; every other error is only the app's refusal.
    pub(crate) async fn refused_refresh(
        &self,
        account: &AccountId,
        error: ProviderError,
    ) -> Refusal {
        let refusal = provider_refusal(error);
        if refusal == Refusal::NeedsReauth {
            self.set_state(account, AccountState::NeedsReauth).await;
        }
        refusal
    }

    pub(crate) async fn issue_token(
        &self,
        caller: &AppId,
        grant: &GrantId,
        audience: &Audience,
    ) -> AccountsReply {
        let found = {
            let registry = self.lock();
            registry.grant_of(caller, grant).and_then(|g| {
                let account = registry.accounts.iter().find(|a| a.id == g.key.account)?;
                Some((
                    account.clone(),
                    g.scope.clone(),
                    g.decision,
                    (g.key.kind, g.key.reach),
                    g.key.space_is_open(),
                ))
            })
        };
        let Some((account, scope, porter_core::consent::Decision::Allow, (kind, reach), open)) =
            found
        else {
            return AccountsReply::Refused(Refusal::UnknownGrant);
        };
        // A grant over another app's own Space is never used, whatever the store holds.
        if !open {
            return AccountsReply::Refused(Refusal::Denied);
        }
        if account.state == AccountState::NeedsReauth {
            return AccountsReply::Refused(Refusal::NeedsReauth);
        }
        let Some(spec) = self.catalog.get(&account.provider) else {
            return AccountsReply::Refused(Refusal::Unavailable);
        };
        if !covers(spec, kind, audience) {
            return AccountsReply::Refused(Refusal::AudienceNotGranted);
        }
        let Some(provider) = self
            .providers
            .iter()
            .find(|p| p.spec().id == account.provider)
        else {
            return AccountsReply::Refused(Refusal::Unavailable);
        };
        // The grant's kind and reach, so the token reaches that kind's service alone, as far as
        // the grant does. One refresh at a time per account; a rotated credential is stored
        // before the token is returned.
        let token = match self
            .issue_for(
                &account,
                provider,
                secret_purpose(account.auth),
                audience,
                (kind, reach),
            )
            .await
        {
            Ok(token) => token,
            Err(refusal) => return AccountsReply::Refused(refusal),
        };
        if scope == GrantScope::Once
            && let Err(refusal) = self.spend_once(caller, grant).await
        {
            return AccountsReply::Refused(refusal);
        }
        self.note(
            Some(caller.clone()),
            Some(account.id.clone()),
            AuditEvent::TokenIssued {
                grant: grant.clone(),
                audience: audience.clone(),
            },
        );
        AccountsReply::Token(token)
    }
}
