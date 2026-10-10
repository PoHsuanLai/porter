//! Which accounts syncd may mirror: the ones it holds a Calendar, Contacts or Tasks grant for.
//!
//! The seam is [`PimGrants`]; [`ClientGrants`] answers it through porter-client's `find`, which
//! lists only grants syncd already holds. **No flow gives syncd such a grant yet**: a consent
//! sheet needs a parent window and a host to draw it (the sheet host, W3e) and a daemon has
//! neither, so today a grant exists when the person makes one in Settings (or when a test
//! seeds one). Until then syncd mirrors nothing and says so on standard error, once per kind.

use super::PimKind;
use porter_client::{Accounts, Found, Transport};
use porter_core::capability::{Access, Delta};
use porter_core::consent::Usage;
use porter_core::need::PimNeed;
use porter_core::{Candidate, Need};
use std::future::Future;
use std::sync::Arc;

/// The need syncd asks accounts to meet: to read the collections and poll them. A task list asks
/// for no more than a full listing can give (`Delta::None`), because Google Tasks has no change
/// token and its feed polls by update time; a list that does better fits too.
pub fn need_of(kind: PimKind) -> Need {
    let need = |delta| PimNeed::new(Access::Read, delta);
    match kind {
        PimKind::Calendar => Need::Calendar(need(Delta::Poll)),
        PimKind::Contacts => Need::Contacts(need(Delta::Poll)),
        PimKind::Tasks => Need::Tasks(need(Delta::None)),
    }
}

/// The granted accounts of a kind.
pub trait PimGrants: Send + Sync {
    /// Every account syncd holds a grant for that can serve `kind`, one candidate each; none
    /// is an empty list. `Err` is that accountd could not be asked (not running, restarting):
    /// nothing is known, so nothing is stopped or removed, and the next look asks again.
    fn granted(
        &self,
        kind: PimKind,
    ) -> impl Future<Output = Result<Vec<Candidate>, AccountdUnavailable>> + Send;
}

/// accountd could not be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("accountd could not be asked")]
pub struct AccountdUnavailable;

/// [`PimGrants`] over an accountd connection.
#[derive(Debug)]
pub struct ClientGrants<T> {
    accounts: Arc<Accounts<T>>,
}

impl<T> ClientGrants<T> {
    /// Grants as `accounts` finds them.
    pub fn new(accounts: Arc<Accounts<T>>) -> Self {
        Self { accounts }
    }
}

impl<T: Transport> PimGrants for ClientGrants<T> {
    async fn granted(&self, kind: PimKind) -> Result<Vec<Candidate>, AccountdUnavailable> {
        let found = self
            .accounts
            .find(&need_of(kind), kind.class(), Usage::Background)
            .await
            .map_err(|_| AccountdUnavailable)?;
        let mut candidates = match found {
            Found::One(one) => vec![one],
            Found::Several(several) => several,
            Found::NeedsConsent(_) | Found::None(_) => Vec::new(),
        };
        let mut seen = std::collections::BTreeSet::new();
        candidates.retain(|c| seen.insert(c.account.clone()));
        Ok(candidates)
    }
}
