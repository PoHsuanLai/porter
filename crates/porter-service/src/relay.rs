//! What a relay presents (`OpenAuthenticated`): the account's password read from the secret
//! store, or an access token the provider session mints, as the account's auth kind says. The
//! value goes into a `RelayPlan` that stays inside the host running the relay; no reply to an
//! app carries it.

use crate::audience::covers;
use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::registry::serves;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::token::{secret_purpose, secrets_refusal};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AppId, Audience, CapabilityKind, Credential, EndpointUrl, Family, GrantId, RelayAuth,
    RelayPlan, SecretKey, SecretPurpose, SecretText, ServiceEndpoint, TokenKind,
};
use porter_provider::{Presented, Provider, ProviderSession};
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
            let after = value.expose().split_once("auth=Bearer ")?.1;
            let token = after.split('\x01').next()?;
            (!token.is_empty()).then(|| SecretText::new(token))
        }
        TokenKind::ApiKeyHandle => None,
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R, A: AuditSink> AccountService<P, S, U, K, R, A>
where
    R: crate::store::RegistryStore,
{
    /// The `RelayPlan` for one checked endpoint of `account`.
    pub(crate) async fn plan_relay(
        &self,
        account: &Account,
        endpoint: &ServiceEndpoint,
        kind: CapabilityKind,
    ) -> Result<RelayPlan, Refusal> {
        let auth = match secret_purpose(account.auth) {
            Some(SecretPurpose::Password) => self.password_for(account, endpoint, kind).await?,
            Some(SecretPurpose::OAuthRefresh) => self.token_for(account, endpoint, kind).await?,
            // An API key or a key pair has no password protocol to present it to.
            _ => return Err(Refusal::Unavailable),
        };
        Ok(RelayPlan::new(endpoint.clone(), kind, auth))
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
    ) -> Result<(Account, ServiceEndpoint, CapabilityKind), Refusal> {
        let (account, kind) = registry.grant_account(caller, grant)?;
        let spec = self
            .catalog
            .get(&account.provider)
            .ok_or(Refusal::EndpointNotGranted)?;
        let dialled = origin.origin();
        let bare = origin.path() == "/" && !origin.as_str().ends_with('/');
        let declared = |endpoint: &ServiceEndpoint| {
            spec.capabilities
                .iter()
                .filter(|row| row.family == endpoint.family && row.capability.kind() == kind)
                .any(|row| row.auth_origins.iter().any(|o| o.allows(&dialled)))
        };
        let home = account
            .endpoints
            .iter()
            .filter(|e| serves(e, kind))
            .find(|e| bare && e.url.origin().scheme == dialled.scheme && declared(e))
            .ok_or(Refusal::EndpointNotGranted)?;
        let endpoint = ServiceEndpoint {
            url: EndpointUrl::parse(&dialled.to_string())
                .map_err(|_| Refusal::EndpointNotGranted)?,
            ..home.clone()
        };
        endpoint.check().map_err(|_| Refusal::EndpointNotGranted)?;
        Ok((account.clone(), endpoint, kind))
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
        let (account, kind) = registry.grant_account(caller, grant)?;
        let spec = self
            .catalog
            .get(&account.provider)
            .ok_or(Refusal::Unavailable)?;
        let dialled = origin.origin();
        let bare = origin.path() == "/" && !origin.as_str().ends_with('/');
        let declared = |endpoint: &ServiceEndpoint| {
            spec.capabilities
                .iter()
                .filter(|row| row.family == endpoint.family && row.capability.kind() == kind)
                .any(|row| row.linked_origins.iter().any(|o| o.allows(&dialled)))
        };
        let home = account
            .endpoints
            .iter()
            .filter(|e| serves(e, kind))
            .find(|e| bare && e.url.origin().scheme == dialled.scheme && declared(e))
            .ok_or(Refusal::EndpointNotGranted)?;
        let endpoint = ServiceEndpoint {
            url: EndpointUrl::parse(&dialled.to_string())
                .map_err(|_| Refusal::EndpointNotGranted)?,
            ..home.clone()
        };
        endpoint.check().map_err(|_| Refusal::EndpointNotGranted)?;
        let plan = RelayPlan::new(endpoint, kind, RelayAuth::Anonymous);
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
        kind: CapabilityKind,
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
        let key = SecretKey {
            account: account.id.clone(),
            purpose: SecretPurpose::OAuthRefresh,
        };
        let credential = self.secrets.get(&key).await.map_err(secrets_refusal)?;
        let session = match provider
            .open(&account.id, Presented::Credential(credential))
            .await
        {
            Ok(session) => session,
            Err(error) => return Err(self.refused_refresh(&account.id, error).await),
        };
        // The grant's kind, so the bearer reaches that kind's service alone.
        let issued = match session.access_token_for(&audience, kind).await {
            Ok(issued) => issued,
            Err(error) => return Err(self.refused_refresh(&account.id, error).await),
        };
        if let Some(renewed) = session.renewed() {
            self.secrets
                .put(&key, &renewed)
                .await
                .map_err(secrets_refusal)?;
        }
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
        let raw = SecretText::new("user=ada\x01auth=Bearer tok-1\x01\x01");
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
