//! What a relay presents (`OpenAuthenticated`): the account's password read from the secret
//! store, or an access token the provider session mints, as the account's auth kind says. The
//! value goes into a `RelayPlan` that stays inside the host running the relay; no reply to an
//! app carries it.

use crate::audience::covers;
use crate::audit::{AuditEvent, AuditSink};
use crate::ctx::Ctx;
use crate::reach::{Declared, declared_endpoint, reach_for};
use crate::sheets::Sheets;
use crate::token::{secret_purpose, secrets_refusal};
use porter_core::capability::StorageScope;
use porter_core::clock::Clock;
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountState, AppId, Audience, CapabilityKind, Credential, EndpointUrl, Family,
    GrantId, RelayAuth, RelayPlan, SecretKey, SecretPurpose, SecretText, ServiceEndpoint,
    TokenKind,
};
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};

/// The secrets a password account may file for `endpoint`, most specific first: a server of its
/// own, then the account's one password.
fn password_purposes(family: Family, kind: CapabilityKind) -> Vec<SecretPurpose> {
    let specific = match family {
        Family::Imap => SecretPurpose::IncomingPassword,
        Family::Smtp => SecretPurpose::OutgoingPassword,
        _ => SecretPurpose::ServicePassword(kind),
    };
    vec![specific, SecretPurpose::Password]
}

/// The bearer inside a token a provider issued as `kind`: a bearer is the token itself; an
/// XOAUTH2 string is `user=..^Aauth=Bearer <token>^A^A`, unencoded (porter-core `TokenKind`).
fn bearer_of(kind: TokenKind, value: &SecretText) -> Option<SecretText> {
    match kind {
        TokenKind::Bearer => Some(value.clone()),
        TokenKind::Xoauth2 => {
            porter_core::token::xoauth2_bearer(value.expose()).map(SecretText::new)
        }
        TokenKind::ApiKeyHandle => None,
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R, A: AuditSink> Ctx<P, S, U, K, R, A>
where
    R: porter_core::store::RegistryStore,
{
    /// The `RelayPlan` for one checked endpoint of `account`.
    pub(crate) async fn plan_relay(
        &self,
        account: &Account,
        endpoint: &ServiceEndpoint,
        (kind, reach): (CapabilityKind, StorageScope),
    ) -> Result<RelayPlan, Refusal> {
        let auth = match secret_purpose(account.auth) {
            Some(SecretPurpose::Password) => self.password_for(account, endpoint, kind).await?,
            Some(SecretPurpose::OAuthRefresh) => {
                self.token_for(account, endpoint, (kind, reach)).await?
            }
            // An API key or a key pair has no password protocol to present it to.
            _ => return Err(Refusal::Unavailable),
        };
        let reach = reach_for(endpoint);
        Ok(RelayPlan::new(endpoint.clone(), kind, auth, reach))
    }

    /// The body of [`AccountService::open_authenticated`](crate::AccountService::open_authenticated).
    pub(crate) async fn open_authenticated(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        let (account, endpoint, kind) = {
            let registry = self.lock();
            match registry.relay_target(caller, grant, endpoint) {
                Ok(target) => (
                    target.account.clone(),
                    target.endpoint.clone(),
                    (target.kind, target.reach),
                ),
                // Not one of the account's endpoints: perhaps an origin its provider file names
                // as one that wants the bearer (picked photos' bytes).
                Err(Refusal::EndpointNotGranted) => {
                    self.auth_origin_target(&registry, caller, grant, endpoint)?
                }
                Err(other) => return Err(other),
            }
        };
        if account.state == AccountState::NeedsReauth {
            return Err(Refusal::NeedsReauth);
        }
        let plan = self.plan_relay(&account, &endpoint, kind).await?;
        self.spend_once(caller, grant).await?;
        self.note(
            Some(caller.clone()),
            Some(account.id.clone()),
            AuditEvent::ProxyOpened {
                grant: grant.clone(),
                endpoint: endpoint.url.clone(),
            },
        );
        Ok(plan)
    }

    /// The body of [`AccountService::open_linked`](crate::AccountService::open_linked).
    pub(crate) async fn open_linked(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        let (plan, account) = self.plan_linked(caller, grant, origin)?;
        self.spend_once(caller, grant).await?;
        self.note(
            Some(caller.clone()),
            Some(account.id.clone()),
            AuditEvent::ProxyOpened {
                grant: grant.clone(),
                endpoint: plan.endpoint.url.clone(),
            },
        );
        Ok(plan)
    }

    /// The account, endpoint and kind for an `OpenAuthenticated` to `origin`, an origin that is
    /// not one of the account's endpoints but that a row of the grant's kind names in
    /// `auth_origins`. The endpoint is the row's own endpoint with the origin's address, so the
    /// bearer is the row family's and nothing else changes. Bare origins only, and only the home
    /// endpoint's scheme; `EndpointNotGranted` for anything else.
    pub(crate) fn auth_origin_target(
        &self,
        registry: &crate::registry::Registry,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<(Account, ServiceEndpoint, (CapabilityKind, StorageScope)), Refusal> {
        let (account, kind, reach) = registry.grant_account(caller, grant)?;
        let spec = self
            .catalog
            .get(&account.provider)
            .ok_or(Refusal::EndpointNotGranted)?;
        let endpoint = declared_endpoint(account, spec, kind, origin, Declared::Auth)?;
        Ok((account.clone(), endpoint, (kind, reach)))
    }

    /// The plan of a relay to `origin`, an origin the account's provider file declares for a
    /// kind of the grant (`linked_origins`) and that is as secure as the endpoint it belongs to
    /// (never `http` for an `https` service). It presents nothing; the origin is all it may dial.
    /// `EndpointNotGranted` for any origin the file does not declare, or one with a path.
    pub(crate) fn plan_linked(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<(RelayPlan, Account), Refusal> {
        let registry = self.lock();
        let (account, kind, _) = registry.grant_account(caller, grant)?;
        let spec = self
            .catalog
            .get(&account.provider)
            .ok_or(Refusal::Unavailable)?;
        let endpoint = declared_endpoint(account, spec, kind, origin, Declared::Linked)?;
        let reach = reach_for(&endpoint);
        let plan = RelayPlan::new(endpoint, kind, RelayAuth::Anonymous, reach);
        Ok((plan, account.clone()))
    }

    async fn password_for(
        &self,
        account: &Account,
        endpoint: &ServiceEndpoint,
        kind: CapabilityKind,
    ) -> Result<RelayAuth, Refusal> {
        for purpose in password_purposes(endpoint.family, kind) {
            let key = SecretKey {
                account: account.id.clone(),
                purpose,
            };
            match self.secrets.get(&key).await {
                Ok(Credential::Password(password)) => return Ok(RelayAuth::Password(password)),
                // A token the person pasted (a JMAP API token): presented as a bearer.
                Ok(Credential::Bearer(token)) => return Ok(RelayAuth::AccessToken(token)),
                Ok(_) => return Err(Refusal::Unavailable),
                Err(SecretsError::Missing) => {}
                Err(error) => return Err(secrets_refusal(error)),
            }
        }
        Err(Refusal::NeedsReauth)
    }

    async fn token_for(
        &self,
        account: &Account,
        endpoint: &ServiceEndpoint,
        (kind, reach): (CapabilityKind, StorageScope),
    ) -> Result<RelayAuth, Refusal> {
        let audience = Audience(endpoint.family.slug().to_owned());
        let spec = self
            .catalog
            .get(&account.provider)
            .ok_or(Refusal::Unavailable)?;
        if !covers(spec, kind, &audience) {
            return Err(Refusal::AudienceNotGranted);
        }
        let provider = self
            .providers
            .iter()
            .find(|p| p.spec().id == account.provider)
            .ok_or(Refusal::Unavailable)?;
        // The grant's kind and reach, so the bearer reaches that kind's service alone, as far as
        // the grant does. One refresh at a time per account, shared with `IssueToken`; a rotated
        // token is stored first.
        let issued = self
            .issue_for(
                account,
                provider,
                Some(SecretPurpose::OAuthRefresh),
                &audience,
                (kind, reach),
            )
            .await?;
        bearer_of(issued.kind, &issued.value)
            .map(RelayAuth::AccessToken)
            .ok_or(Refusal::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bearer_comes_out_of_whichever_form_the_provider_issued() {
        let raw = porter_core::token::xoauth2("ada", "tok-1");
        let plain = SecretText::new("tok-3");
        let cases = [
            (TokenKind::Bearer, &plain, Some("tok-3")),
            (TokenKind::Xoauth2, &raw, Some("tok-1")),
            (TokenKind::Xoauth2, &plain, None),
            (TokenKind::ApiKeyHandle, &plain, None),
        ];
        for (kind, value, expected) in cases {
            assert_eq!(
                bearer_of(kind, value).as_ref().map(SecretText::expose),
                expected,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_server_with_its_own_password_is_tried_before_the_accounts() {
        assert_eq!(
            password_purposes(Family::Imap, CapabilityKind::Mail),
            [SecretPurpose::IncomingPassword, SecretPurpose::Password]
        );
        assert_eq!(
            password_purposes(Family::Smtp, CapabilityKind::Mail)[0],
            SecretPurpose::OutgoingPassword
        );
        assert_eq!(
            password_purposes(Family::CardDav, CapabilityKind::Contacts)[0],
            SecretPurpose::ServicePassword(CapabilityKind::Contacts)
        );
    }
}
