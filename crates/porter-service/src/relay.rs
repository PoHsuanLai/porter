//! What a relay presents (`OpenAuthenticated`): the account's password read from the secret
//! store, or an access token the provider session mints, as the account's auth kind says. The
//! value goes into a `RelayPlan` that stays inside the host running the relay; no reply to an
//! app carries it.

use crate::audience::covers;
use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::token::{provider_refusal, secret_purpose, secrets_refusal};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::wire::Refusal;
use porter_core::{
    Account, Audience, CapabilityKind, Credential, Family, RelayAuth, RelayPlan, SecretKey,
    SecretPurpose, SecretText, ServiceEndpoint, TokenKind,
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
/// XOAUTH2 string is `user=..^Aauth=Bearer <token>^A^A`, plain or base64.
fn bearer_of(kind: TokenKind, value: &SecretText) -> Option<SecretText> {
    match kind {
        TokenKind::Bearer => Some(value.clone()),
        TokenKind::Xoauth2 => {
            let text = value.expose();
            let decoded = STANDARD
                .decode(text)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok());
            [Some(text.to_owned()), decoded]
                .into_iter()
                .flatten()
                .find_map(|candidate| {
                    let after = candidate.split_once("auth=Bearer ")?.1;
                    let token = after.split('\x01').next()?;
                    (!token.is_empty()).then(|| SecretText::new(token))
                })
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
        Ok(RelayPlan {
            endpoint: endpoint.clone(),
            kind,
            auth,
        })
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
        let session = provider
            .open(&account.id, Presented::Credential(credential))
            .await
            .map_err(provider_refusal)?;
        let issued = session
            .access_token(&audience)
            .await
            .map_err(provider_refusal)?;
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
        let wrapped = SecretText::new(STANDARD.encode("user=ada\x01auth=Bearer tok-2\x01\x01"));
        let plain = SecretText::new("tok-3");
        let cases = [
            (TokenKind::Bearer, &plain, Some("tok-3")),
            (TokenKind::Xoauth2, &raw, Some("tok-1")),
            (TokenKind::Xoauth2, &wrapped, Some("tok-2")),
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
