//! The key an account's consent decision is stored under: this app, this account, this kind,
//! this data class, this use, which Spaces, and for Storage how far into the store. It is the
//! accounts' key for porter-core's consent, which is generic over the key it decides on and
//! names no Space; the Space ownership rule is decided here.

use crate::AccountId;
use crate::app_id::AppId;
use crate::consent::{Grant, Usage, Verdict, decide};
use crate::data_class::DataClass;
use crate::space::SpaceScope;
use porter_capability::{CapabilityKind, StorageScope};
use serde::{Deserialize, Serialize};

/// What one decision covers: this app, this account, this kind, this data class, this use, and
/// for Storage how far into the store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GrantKey {
    /// The app.
    pub app: AppId,
    /// The account.
    pub account: AccountId,
    /// The capability kind.
    pub kind: CapabilityKind,
    /// The data class.
    pub class: DataClass,
    /// Interactive or background use.
    pub usage: Usage,
    /// The Spaces it covers: account grants made from the sheet cover `Any`.
    pub space: SpaceScope,
    /// How far a Storage grant reaches, from the need it was made for (`Need::reach`): the app
    /// folder, or the whole store, whose token carries the provider's on-use row. The app folder
    /// for every other kind, and for a grant stored before grants had a reach.
    #[serde(default = "app_folder")]
    pub reach: StorageScope,
}

fn app_folder() -> StorageScope {
    StorageScope::AppFolder
}

impl GrantKey {
    /// Whether the key's Space may be decided for its app: an app's own Space belongs to that
    /// app alone, so a key of one app over another's own Space is never granted. Checked against
    /// the key's `app`, which is the caller's verified identity, never against a request's text.
    pub fn space_is_open(&self) -> bool {
        self.space.open_to(&self.app.name)
    }
}

/// [`decide`] for an account key, with the Space ownership rule first: a key of one app over
/// another app's own Space is `Denied` whatever the store holds, and is never asked.
pub fn decide_key(grants: &[Grant], key: &GrantKey) -> Verdict {
    match key.space_is_open() {
        true => decide(grants, key),
        false => Verdict::Denied,
    }
}

#[cfg(test)]
mod tests;
