//! The grant that lets syncd keep an account's storage on this computer (Settings' "Keep this
//! account's files on this computer" and "Back up photos"). syncd is a porter daemon, an app of
//! its own as far as consent goes (`org.quire.Sync`); its mirrors run only for an account it
//! holds a `Storage` grant for, with usage `Background` and the class of the data (`Files` for
//! the app folder, `Photos` for the Photos datasets). No sheet can ask the person for this
//! grant (a daemon has no window), so Settings makes it and takes it back.
//!
//! Photos on a Google account is the one exception to the `Storage` kind: Google Photos is not a
//! store syncd mirrors, it is an upload target and a picker import with APIs of their own, and
//! their audiences (`google_photos_upload`, `google_photos_picker`) belong to the `Photos` kind
//! of the provider file. syncd's key for it is `Photos`-kind, class `Photos` ([`photos_key`]),
//! so a `Storage` grant of syncd's (the Drive app folder) never reaches them.

use crate::audit::AuditSink;
use crate::choose::next_grant_id;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::audit::AuditEvent;
use porter_core::capability::Offered;
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountId, AppId, AppName, Capability, CapabilityKind, DataClass, Family, Isolation,
    Offer, SpaceScope, Toggle,
};
use porter_provider::Provider;
use porter_secrets::Secrets;

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

/// The key of the grant that lets syncd keep `class` of `account`: [`sync_key`], except for the
/// Photos of an account whose photos are Google's ([`photos_key`]).
pub fn sync_key_of(account: &Account, class: SyncClass) -> GrantKey {
    match (class, google_photos(account)) {
        (SyncClass::Photos, true) => photos_key(&account.id),
        _ => sync_key(&account.id, class),
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
fn keys_of(account: &AccountId, class: SyncClass) -> Vec<GrantKey> {
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

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Lets syncd keep `class` of `account` (an always-allow, background grant for
    /// `org.quire.Sync`) or takes that back. `On` twice is one grant; `Off` with none is
    /// nothing. An account that offers nothing for syncd to keep is `NoFittingAccount`, an
    /// unknown one `UnknownGrant`. The mirror's files stay when the grant goes.
    pub async fn set_sync(
        &self,
        account: &AccountId,
        class: SyncClass,
        toggle: Toggle,
    ) -> Result<(), Refusal> {
        let (changed, made, taken, kind) = {
            let mut registry = self.lock();
            let row = registry
                .accounts
                .iter()
                .find(|a| a.id == *account)
                .ok_or(Refusal::UnknownGrant)?;
            let key = sync_key_of(row, class);
            let kind = key.kind;
            let offered = sync_offers(row).contains(&class);
            match toggle {
                Toggle::On if !offered => return Err(Refusal::NoFittingAccount),
                Toggle::On if sync_allowed(&registry.grants, account, class) => {
                    (false, None, Vec::new(), kind)
                }
                Toggle::On => {
                    // A refusal kept for this key would outrank nothing here: the person's
                    // newest word is this switch.
                    let taken: Vec<Grant> = registry
                        .grants
                        .iter()
                        .filter(|g| g.key == key)
                        .cloned()
                        .collect();
                    registry.grants.retain(|g| g.key != key);
                    let id = next_grant_id(&registry);
                    registry.grants.push(Grant {
                        id: id.clone(),
                        key,
                        decision: Decision::Allow,
                        scope: GrantScope::Always,
                        at: self.clock.now(),
                    });
                    (true, Some(id), taken, kind)
                }
                Toggle::Off => {
                    // Both keys a class may have: a switch off takes back whichever is held.
                    let wanted = keys_of(account, class);
                    let taken: Vec<Grant> = registry
                        .grants
                        .iter()
                        .filter(|g| wanted.contains(&g.key))
                        .cloned()
                        .collect();
                    registry.grants.retain(|g| !wanted.contains(&g.key));
                    (!taken.is_empty(), None, taken, kind)
                }
            }
        };
        if !changed {
            return Ok(());
        }
        for grant in taken {
            self.note(
                Some(grant.key.app),
                Some(grant.key.account),
                AuditEvent::Revoked { grant: grant.id },
            );
        }
        if let Some(grant) = made {
            self.note(
                Some(sync_app()),
                Some(account.clone()),
                AuditEvent::Granted { grant, kind },
            );
        }
        self.persist().await
    }
}
