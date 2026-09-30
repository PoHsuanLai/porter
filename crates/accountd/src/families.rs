//! The protocol families as code: one variant per built family (`porter_provider::Family`),
//! each wrapping its engine. None is built yet, so the set is empty and uninhabited.

use porter_core::{AccountId, Audience, Claim, Credential, IssuedToken};
use porter_provider::{Presented, Provider, ProviderError, ProviderSession, ProviderSpec};

/// Every built family.
#[derive(Debug)]
pub(crate) enum FamilyProvider {}

/// An open account of a built family.
#[derive(Debug)]
pub(crate) enum FamilySession {}

impl Provider for FamilyProvider {
    type Session = FamilySession;

    fn spec(&self) -> &ProviderSpec {
        match *self {}
    }

    async fn discover(&self, _: &AccountId, _: &Presented) -> Result<Vec<Claim>, ProviderError> {
        match *self {}
    }

    async fn open(&self, _: &AccountId, _: Presented) -> Result<FamilySession, ProviderError> {
        match *self {}
    }
}

impl ProviderSession for FamilySession {
    async fn access_token(&self, _: &Audience) -> Result<IssuedToken, ProviderError> {
        match *self {}
    }

    fn renewed(&self) -> Option<Credential> {
        match *self {}
    }
}
