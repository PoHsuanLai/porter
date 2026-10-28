//! The rules of an OAuth issuer that has none of its own: its data says everything. A sign-in
//! asks the data's scopes for the kinds the provider file declares, the address is read from the
//! issuer's userinfo endpoint, and what the file declares is what the account offers (there is no
//! per-service probe to ask). Tokens are not narrowed per kind: a grant is the whole of what was
//! granted, for the audiences the file names.

use super::flow::{Cx, Grant, provider_fault};
use super::issuer::BrowserIssuer;
use super::session::OAuthSession;
use super::userinfo_address;
use porter_capability::{CapabilityKind, Claim, Offer, Provenance, StorageScope, Subject};
use porter_core::{
    AccountLabel, Audience, Credential, EndpointUrl, GrantedScopes, LoginName, Restriction,
    SecretPurpose, ServiceEndpoint, Tls, TokenKind,
};
use porter_http::Http;
use porter_ids::UrlScheme;
use porter_oauth::TokenResponse;
use porter_provider::{Issuer, ProviderError, ProviderSpec, Signed};
use porter_sheet::SignInFault;

/// The rules of `issuer`, which publishes who answered at its userinfo endpoint and has nothing
/// else to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standard(Issuer);

impl Standard {
    /// Whether `issuer` needs no rules of its own: its data names a userinfo endpoint.
    pub fn serves(issuer: Issuer) -> bool {
        issuer.data().userinfo.is_some()
    }

    /// The rules of `issuer`.
    pub fn of(issuer: Issuer) -> Self {
        Self(issuer)
    }
}

impl BrowserIssuer for Standard {
    type State = ();

    fn issuer(&self) -> Issuer {
        self.0
    }

    fn open_state(&self, _credential: &Credential) {}

    async fn signed<H: Http + 'static>(
        &self,
        cx: &Cx<'_, H>,
        grant: Grant,
        tokens: TokenResponse,
    ) -> Result<Signed, SignInFault> {
        let now = (cx.env.clock)();
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(SignInFault::Unreadable)?;
        let userinfo = cx.env.userinfo_of(self.0).ok_or(SignInFault::Unreadable)?;
        let address = userinfo_address(&*cx.env.http, &userinfo, tokens.access_token.expose())
            .await
            .map_err(provider_fault)?;
        Ok(Signed::new(
            AccountLabel(address.clone()),
            vec![(
                SecretPurpose::OAuthRefresh,
                Credential::OAuth {
                    expires_at: tokens.expires_at(now),
                    access: tokens.access_token,
                    refresh,
                    granted: GrantedScopes::of(grant.scopes),
                },
            )],
            declared_claims(cx.spec),
            declared_endpoints(cx.spec, &address),
            Restriction::none(),
        ))
    }

    async fn extended<H: Http + 'static>(
        &self,
        cx: &Cx<'_, H>,
        grant: &Grant,
        tokens: TokenResponse,
        stored: &[ServiceEndpoint],
        kinds: &[CapabilityKind],
    ) -> Result<Signed, SignInFault> {
        let now = (cx.env.clock)();
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(SignInFault::Unreadable)?;
        let userinfo = cx.env.userinfo_of(self.0).ok_or(SignInFault::Unreadable)?;
        let address = userinfo_address(&*cx.env.http, &userinfo, tokens.access_token.expose())
            .await
            .map_err(provider_fault)?;
        let mut granted = self
            .0
            .data()
            .sign_in_scopes(&cx.spec.sign_in_kinds(), false);
        granted.extend(grant.scopes.iter().cloned());
        Ok(Signed::new(
            AccountLabel(address.clone()),
            vec![(
                SecretPurpose::OAuthRefresh,
                Credential::OAuth {
                    expires_at: tokens.expires_at(now),
                    access: tokens.access_token,
                    refresh,
                    granted: GrantedScopes::of(granted),
                },
            )],
            crate::extend::asked_claims(cx.spec, kinds),
            crate::extend::answered_as(stored, &address),
            Restriction::none(),
        ))
    }

    async fn discover<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> Result<Vec<Claim>, ProviderError> {
        Ok(declared_claims(session.spec()))
    }

    async fn identify<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> Result<String, ProviderError> {
        let userinfo = session
            .env()
            .userinfo_of(self.0)
            .ok_or(ProviderError::Unreadable)?;
        let token = session.mint(None).await?;
        userinfo_address(&*session.env().http, &userinfo, token.value.expose()).await
    }

    fn porter_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
    ) -> Result<(Option<String>, TokenKind), ProviderError> {
        Ok((None, form_of(session.spec(), audience)?))
    }

    fn grant_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
        kind: CapabilityKind,
        _reach: StorageScope,
    ) -> Result<(Option<String>, TokenKind), ProviderError> {
        // The audience must be one the file declares for this kind.
        let wanted = audience_word(audience);
        let named = session
            .spec()
            .capabilities
            .iter()
            .filter(|row| row.capability.kind() == kind)
            .any(|row| names(row, &wanted));
        match named {
            true => Ok((None, form_of(session.spec(), audience)?)),
            false => Err(ProviderError::Forbidden),
        }
    }
}

fn audience_word(audience: &Audience) -> String {
    audience.0.trim_end_matches('/').to_owned()
}

fn names(row: &porter_provider::CapabilityRow, wanted: &str) -> bool {
    porter_ids::word::word(&row.family) == wanted
        || row
            .endpoint
            .as_ref()
            .is_some_and(|e| e.0.trim_end_matches('/') == wanted)
}

/// The form of token `audience` takes, or `Forbidden` for one that is not the account's: IMAP
/// and SMTP take an XOAUTH2 string, anything else a row of the file names a bearer.
fn form_of(spec: &ProviderSpec, audience: &Audience) -> Result<TokenKind, ProviderError> {
    let wanted = audience_word(audience);
    if wanted == "imap" || wanted == "smtp" {
        return Ok(TokenKind::Xoauth2);
    }
    match spec.capabilities.iter().any(|row| names(row, &wanted)) {
        true => Ok(TokenKind::Bearer),
        false => Err(ProviderError::Forbidden),
    }
}

/// What the file declares, as the account offers it.
fn declared_claims(spec: &ProviderSpec) -> Vec<Claim> {
    spec.sign_in_kinds()
        .into_iter()
        .filter_map(|kind| {
            spec.sign_in_rows()
                .find(|row| row.capability.kind() == kind)
        })
        .map(|row| Claim {
            subject: Subject::Account,
            offer: Offer::Present(row.capability.clone()),
            provenance: Provenance::Declared,
        })
        .collect()
}

/// The servers the file names, signed in as `address`.
fn declared_endpoints(spec: &ProviderSpec, address: &str) -> Vec<ServiceEndpoint> {
    spec.sign_in_rows()
        .filter_map(|row| {
            let url = EndpointUrl::parse(&row.endpoint.as_ref()?.0).ok()?;
            let tls = match url.origin().scheme {
                UrlScheme::Imap | UrlScheme::Smtp => Tls::StartTls,
                UrlScheme::Http => Tls::Plain,
                _ => Tls::Implicit,
            };
            Some(ServiceEndpoint {
                family: row.family,
                url,
                tls,
                login: LoginName(address.to_owned()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issuer_with_a_userinfo_endpoint_needs_no_rules_of_its_own() {
        assert!(Standard::serves(Issuer::Yahoo));
        assert!(!Standard::serves(Issuer::Dropbox));
        assert!(!Standard::serves(Issuer::OpenRouter));
    }
}
