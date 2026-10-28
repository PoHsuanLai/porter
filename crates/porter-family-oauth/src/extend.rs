//! What the OAuth families share for a sign-in that asks for one more access
//! (`SignInMode::Extend`): the review of what it asks, and who answered.

use porter_capability::CapabilityKind;
use porter_capability::{Claim, Offer, Provenance, Subject};
use porter_core::{LoginName, ServiceEndpoint};
use porter_provider::ProviderSpec;

/// What the review of an extension shows: each on-use row of `kinds` as offered, so the sheet
/// names the access in plain words (`Capability::access_words`).
pub(crate) fn asked_claims(spec: &ProviderSpec, kinds: &[CapabilityKind]) -> Vec<Claim> {
    kinds
        .iter()
        .filter_map(|kind| spec.on_use_rows(*kind).next())
        .map(|row| Claim {
            subject: Subject::Account,
            offer: Offer::Present(row.capability.clone()),
            provenance: Provenance::Declared,
        })
        .collect()
}

/// The account's servers as stored, signed in as `address`, the person who answered: the host
/// keeps the answer only when that is the login it holds.
pub(crate) fn answered_as(stored: &[ServiceEndpoint], address: &str) -> Vec<ServiceEndpoint> {
    stored
        .iter()
        .map(|endpoint| ServiceEndpoint {
            login: LoginName(address.to_owned()),
            ..endpoint.clone()
        })
        .collect()
}
