//! Which Microsoft accounts syncd may mirror Storage from: the ones it holds a Storage grant
//! for, by what the grant is for ([`StorageKind`]). Like the PIM grants, the answer comes from
//! `Accounts::find`, which lists only grants syncd already holds: a grant exists when the person
//! makes one in Settings (or a test seeds one), since a daemon cannot draw a consent sheet.

use crate::datasets::pim::AccountdUnavailable;
use porter_client::{Accounts, Found, Transport};
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::Usage;
use porter_core::need::StorageNeed;
use porter_core::{Candidate, DataClass, Family, Need};
use std::future::Future;
use std::sync::Arc;

/// What a Storage grant of syncd's is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StorageKind {
    /// The account's app folder, mirrored two ways into a local folder (`DataClass::Files`).
    AppFolder,
    /// The Photos dataset's two folders in the app folder (`DataClass::Photos`), behind the
    /// [`crate::datasets::photos::PhotosSwitch`].
    Photos,
}

impl StorageKind {
    /// Every kind.
    pub const ALL: [StorageKind; 2] = [StorageKind::AppFolder, StorageKind::Photos];

    /// The data class the grant covers.
    pub fn class(self) -> DataClass {
        match self {
            StorageKind::AppFolder => DataClass::Files,
            StorageKind::Photos => DataClass::Photos,
        }
    }
}

/// The need syncd asks accounts to meet: read and write the app folder and poll it.
pub fn storage_need() -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    })
}

/// Whether the candidate is reached over Graph (the only Storage mirror syncd runs): the
/// endpoint to dial.
pub fn graph_endpoint(candidate: &Candidate) -> Option<&porter_core::ServiceEndpoint> {
    candidate
        .endpoints
        .iter()
        .find(|e| e.family == Family::Graph)
}

/// The granted accounts of a kind.
pub trait StorageGrants: Send + Sync {
    /// Every account syncd holds a Storage grant of this kind for that is reached over Graph,
    /// one candidate each; none is an empty list. `Err` is that accountd could not be asked:
    /// nothing is known, so nothing is stopped, and the next look asks again.
    fn granted(
        &self,
        kind: StorageKind,
    ) -> impl Future<Output = Result<Vec<Candidate>, AccountdUnavailable>> + Send;
}

/// [`StorageGrants`] over an accountd connection.
#[derive(Debug)]
pub struct ClientStorageGrants<T> {
    accounts: Arc<Accounts<T>>,
}

impl<T> ClientStorageGrants<T> {
    /// Grants as `accounts` finds them.
    pub fn new(accounts: Arc<Accounts<T>>) -> Self {
        Self { accounts }
    }
}

impl<T: Transport> StorageGrants for ClientStorageGrants<T> {
    async fn granted(&self, kind: StorageKind) -> Result<Vec<Candidate>, AccountdUnavailable> {
        let found = self
            .accounts
            .find(&storage_need(), kind.class(), Usage::Background)
            .await
            .map_err(|_| AccountdUnavailable)?;
        let mut candidates = match found {
            Found::One(one) => vec![one],
            Found::Several(several) => several,
            Found::NeedsConsent(_) | Found::None(_) => Vec::new(),
        };
        let mut seen = std::collections::BTreeSet::new();
        candidates.retain(|c| graph_endpoint(c).is_some() && seen.insert(c.account.clone()));
        Ok(candidates)
    }
}
