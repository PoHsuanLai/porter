//! A provider that answers from its declaration and mints predictable tokens.

use porter_core::{
    AccountId, Audience, Claim, IssuedToken, Offer, Provenance, SecretText, Subject, TokenKind,
    UnixSeconds,
};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, parse_provider,
};

/// A provider over one parsed provider file.
#[derive(Debug, Clone)]
pub struct FakeProvider {
    spec: ProviderSpec,
}

impl FakeProvider {
    /// The provider the file text declares. Panics on a bad file: the files are this crate's.
    pub fn from_file(text: &str) -> Self {
        let spec = parse_provider(text).unwrap_or_else(|e| panic!("fake provider file: {e}"));
        Self { spec }
    }
}

/// The self-hosted cloud (Storage + Calendar, app password).
pub fn cloud_provider() -> FakeProvider {
    FakeProvider::from_file(include_str!("../providers/fake-cloud.toml"))
}

/// The mail host (Mail, OAuth).
pub fn mail_provider() -> FakeProvider {
    FakeProvider::from_file(include_str!("../providers/fake-mail.toml"))
}

/// The local runtime (Llm, on device).
pub fn llm_provider() -> FakeProvider {
    FakeProvider::from_file(include_str!("../providers/fake-llm.toml"))
}

/// An open fake account.
#[derive(Debug, Clone)]
pub struct FakeSession {
    account: AccountId,
}

impl Provider for FakeProvider {
    type Session = FakeSession;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        _account: &AccountId,
        _presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        Ok(self
            .spec
            .capabilities
            .iter()
            .map(|row| Claim {
                subject: Subject::Account,
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Discovered,
            })
            .collect())
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<FakeSession, ProviderError> {
        let needs_secret = porter_service::secret_purpose(self.auth_kind()).is_some();
        match (needs_secret, presented) {
            (true, Presented::Anonymous) => Err(ProviderError::Unauthorized),
            _ => Ok(FakeSession {
                account: account.clone(),
            }),
        }
    }
}

impl ProviderSession for FakeSession {
    async fn access_token(&self, audience: &Audience) -> Result<IssuedToken, ProviderError> {
        Ok(IssuedToken {
            kind: TokenKind::Bearer,
            value: SecretText::new(format!("fake:{}:{}", self.account, audience.0)),
            expires: UnixSeconds(crate::world::NOW.0 + 3600),
        })
    }

    fn renewed(&self) -> Option<porter_core::Credential> {
        None
    }
}
