//! A provider that answers from its declaration and mints predictable tokens.

use porter_core::sheet::{SignInFault, SignInInput};
use porter_core::{
    AccountId, AccountLabel, Audience, Claim, Credential, IssuedToken, Offer, Provenance,
    Restriction, SecretPurpose, SecretText, Subject, TokenKind, UnixSeconds,
};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignIn,
    SignInStart, SignInStep, Signed, parse_provider,
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

/// A sign-in that needs nothing from the person: `Start` signs in at once with a fixed
/// password, and anything after it is a refusal. A real family asks, waits and reviews.
#[derive(Debug)]
pub struct FakeSignIn {
    label: AccountLabel,
    claims: Vec<Claim>,
}

impl SignIn for FakeSignIn {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        match input {
            SignInInput::Start => SignInStep::Done(Signed {
                label: self.label.clone(),
                credentials: vec![(
                    SecretPurpose::Password,
                    Credential::Password(SecretText::new("fake-signed-in")),
                )],
                claims: self.claims.clone(),
                endpoints: Vec::new(),
                restriction: Restriction::none(),
            }),
            _ => SignInStep::Failed(SignInFault::Cancelled),
        }
    }
}

impl Provider for FakeProvider {
    type Session = FakeSession;
    type SignIn = FakeSignIn;

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

    fn sign_in(&self, _start: SignInStart) -> Result<FakeSignIn, ProviderError> {
        Ok(FakeSignIn {
            label: AccountLabel(self.spec.label.clone()),
            claims: self
                .spec
                .capabilities
                .iter()
                .map(|row| Claim {
                    subject: Subject::Account,
                    offer: Offer::Present(row.capability.clone()),
                    provenance: Provenance::Declared,
                })
                .collect(),
        })
    }

    async fn revoke(&self, _presented: &Presented) -> Result<RevokeOutcome, ProviderError> {
        Ok(RevokeOutcome::Revoked)
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
