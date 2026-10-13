//! What syncd may keep of an account, and the grant that says so. syncd is a daemon of its own as
//! far as consent goes (`org.quire.Sync`): its mirrors run only for an account it holds a
//! `Storage` grant for, with usage `Background` and the class of the data (`Files` for the app
//! folder, `Photos` for the Photos datasets). Nothing asks the person for these grants as a
//! consent sheet: turning the sync switch on in Settings is the person's answer.
//!
//! Photos on a Google account is the one exception to the `Storage` kind: Google Photos is an
//! upload target and a picker import with APIs of their own, and their audiences belong to the
//! `Photos` kind of the provider file. syncd's key for it is `Photos`-kind, class `Photos`
//! ([`photos_key`]), so a `Storage` grant of syncd's (the Drive app folder) never reaches them.

use crate::datasets::storage::{photos_need, storage_need};
use porter_core::capability::Offered;
use porter_core::consent::{Decision, Grant, GrantKey, Usage};
use porter_core::{
    Account, AccountId, AppId, AppName, Capability, CapabilityKind, DataClass, Family, Isolation,
    Need, Offer, SpaceScope,
};

/// What syncd is allowed to keep of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncClass {
    /// The account's app folder, mirrored both ways.
    Files,
    /// The Photos originals and their metadata.
    Photos,
}

impl SyncClass {
    /// Both, in the order Settings lists them.
    pub const ALL: [SyncClass; 2] = [SyncClass::Files, SyncClass::Photos];

    /// The class of data the grant is for.
    pub fn class(self) -> DataClass {
        match self {
            SyncClass::Files => DataClass::Files,
            SyncClass::Photos => DataClass::Photos,
        }
    }

    /// The word a settings key ends in.
    pub fn slug(self) -> &'static str {
        match self {
            SyncClass::Files => "files",
            SyncClass::Photos => "photos",
        }
    }
}

/// syncd as an app of consent: a native daemon, as the caller table names it.
pub fn sync_app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Sync").expect("a literal app name"),
        isolation: Isolation::Unsandboxed,
    }
}

/// The key of the grant that lets syncd keep `class` of `account`.
pub fn sync_key(account: &AccountId, class: SyncClass) -> GrantKey {
    GrantKey {
        app: sync_app(),
        account: account.clone(),
        kind: CapabilityKind::Storage,
        class: class.class(),
        usage: Usage::Background,
        space: SpaceScope::Any,
    }
}

/// The key of the grant that lets syncd upload to, and import from, the Google Photos of
/// `account`: `Photos` kind, class `Photos`.
pub fn photos_key(account: &AccountId) -> GrantKey {
    GrantKey {
        kind: CapabilityKind::Photos,
        ..sync_key(account, SyncClass::Photos)
    }
}

fn has_endpoint(account: &Account, family: Family) -> bool {
    account.endpoints.iter().any(|e| e.family == family)
}

/// Whether `account` offers Google Photos uploads (a Photos capability that uploads, and the
/// upload API's endpoint).
fn google_photos(account: &Account) -> bool {
    let uploads = account.capabilities.iter().any(|claim| {
        matches!(&claim.offer, Offer::Present(Capability::Photos(cap)) if cap.upload == Offered::Present)
    });
    uploads && has_endpoint(account, Family::GooglePhotosUpload)
}

/// What syncd can keep of `account`: the app folder when the account offers Storage over Graph
/// or the Google Drive app data folder (the stores syncd mirrors), and Photos besides when that
/// store takes resumable uploads, which original photos need; for a Google account, Photos is
/// the upload target and the picker import, whatever its Drive offers.
pub fn sync_offers(account: &Account) -> Vec<SyncClass> {
    let storage = account
        .capabilities
        .iter()
        .find_map(|claim| match &claim.offer {
            Offer::Present(Capability::Storage(cap)) => Some(cap),
            _ => None,
        });
    let graph = has_endpoint(account, Family::Graph);
    let drive = has_endpoint(account, Family::GoogleDrive);
    let mut offered = Vec::new();
    match storage {
        Some(cap) if graph => {
            offered.push(SyncClass::Files);
            if cap.chunked_upload == Offered::Present {
                offered.push(SyncClass::Photos);
            }
        }
        Some(_) if drive => offered.push(SyncClass::Files),
        _ => {}
    }
    if google_photos(account) {
        offered.push(SyncClass::Photos);
    }
    offered
}

/// Every key a grant for `class` of `account` may have: the Storage one, and for Photos the
/// Photos-kind one of a Google account.
pub fn keys_of(account: &AccountId, class: SyncClass) -> Vec<GrantKey> {
    let mut keys = vec![sync_key(account, class)];
    if class == SyncClass::Photos {
        keys.push(photos_key(account));
    }
    keys
}

/// Whether syncd holds an allowing grant for `class` of `account`: under either key it may
/// have (the Storage one, or the Photos one of a Google account).
pub fn sync_allowed(grants: &[Grant], account: &AccountId, class: SyncClass) -> bool {
    let wanted = keys_of(account, class);
    grants
        .iter()
        .any(|g| wanted.contains(&g.key) && g.decision == Decision::Allow)
}

/// The need syncd's grant for `class` of `account` is asked under: the app folder's for Files,
/// Google Photos' for the Photos of a Google account, the Photos folders' (Graph) otherwise.
pub fn need_for(account: &Account, class: SyncClass) -> Need {
    match (class, google_photos(account)) {
        (SyncClass::Photos, true) => photos_need(),
        _ => storage_need(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::GrantId;
    use porter_core::UnixSeconds;
    use porter_core::consent::GrantScope;

    fn grant(key: GrantKey, decision: Decision) -> Grant {
        Grant {
            id: GrantId::parse("g1").expect("id"),
            key,
            decision,
            scope: GrantScope::Always,
            at: UnixSeconds(1),
        }
    }

    #[test]
    fn a_class_is_allowed_only_by_an_allowing_grant_under_one_of_its_keys() {
        let id = AccountId::parse("cloud").expect("id");
        let other = AccountId::parse("other").expect("id");
        let cases = [
            (vec![], SyncClass::Files, false),
            (
                vec![grant(sync_key(&id, SyncClass::Files), Decision::Allow)],
                SyncClass::Files,
                true,
            ),
            (
                vec![grant(sync_key(&id, SyncClass::Files), Decision::Deny)],
                SyncClass::Files,
                false,
            ),
            (
                vec![grant(sync_key(&id, SyncClass::Files), Decision::Allow)],
                SyncClass::Photos,
                false,
            ),
            (
                vec![grant(photos_key(&id), Decision::Allow)],
                SyncClass::Photos,
                true,
            ),
            (
                vec![grant(sync_key(&other, SyncClass::Files), Decision::Allow)],
                SyncClass::Files,
                false,
            ),
        ];
        for (grants, class, want) in cases {
            assert_eq!(sync_allowed(&grants, &id, class), want, "{class:?}");
        }
    }
}
