//! syncd's own accounts. Whoever uses a credential holds it: syncd signs in for itself ("Continue
//! as") and keeps its own tokens, in its own registry (`$XDG_STATE_HOME/quire/syncd/keys`) and its
//! own keyring namespace (`porter.syncd`), in a service it hosts in process. accountd only says
//! which identities the person has signed in and shows syncd as one of the apps using one.
//!
//! - [`Keep::continue_as`]: the person picks an identity in accountd's sheet
//!   ([`Directory::offer`]), syncd signs that address in through the sheet host
//!   (`AccountService::sign_in_as`, opening the provider's page on the address), then tells
//!   accountd it uses the identity ([`Directory::register_use`]).
//! - [`Keep::set_sync`]: the sync switches. On records syncd's own grant for the class (the
//!   switch is the person's answer: [`SyncSheets`] answers syncd's consent ask from it), off takes
//!   it back, and the mirror stops at the supervisor's next look; the files stay.
//! - [`Keep::show_in_files`]: "Show in Files". On asks the person for the whole drive in a
//!   sign-in of its own when the account was not given it yet ([`Hosted::extend_access`]), then
//!   records syncd's grant of it, and the supervisor lists the drive at its next look; off takes
//!   the grant back and keeps the listing, and the access stays given, so on again asks nothing.
//! - [`Keep::keep_everything`]: "Keep Everything Downloaded" for a drive shown in Files, kept in
//!   syncd's settings so it holds across restarts (the drive does the keeping:
//!   `porter_drive::Drive::set_keep_all`).
//! - [`Keep::remove`]: forgets an account: stops its datasets, deletes its journals and mirrors
//!   ([`crate::removal::wipe`]) and its secrets. accountd removing the identity does not do this:
//!   the person removes it in the sync rows.

mod audit;
mod classes;
mod hosted;

pub use audit::FileAudit;
pub use classes::{
    SyncClass, claims_whole_drive, drive_allowed, drive_key, keys_of, lists_whole_drive, need_for,
    photos_key, sync_allowed, sync_app, sync_key, sync_offers,
};
pub use hosted::{Directory, Hosted, Intent, Pick, SyncSheets, account_of};

use crate::datasets::drive::{DriveSet, Kept};
use crate::paths::{AccountDir, Paths};
use crate::removal;
use crate::service::Hub;
use porter_capability::{CapabilityKind, Toggle};
use porter_client::IdentitiesError;
use porter_core::AccountId;
use porter_core::GrantKey;
use porter_core::consent::Usage;
use porter_core::{Account, DataClass};
use porter_provider::{ConsentHistory, LoginHint};
use porter_service_accounts::SignInAs;
use porter_wire::{AccountsReply, AccountsRequest};
use porter_wire::{ParentWindow, Refusal};

/// Why "Continue as" did not end in an account.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContinueError {
    /// The person closed the sheet without picking, or without finishing the sign-in.
    #[error("the sign-in was closed before it finished")]
    Cancelled,
    /// No account is signed in on this computer to continue as.
    #[error("there is no account to continue as")]
    NoIdentity,
    /// The picked account's address is not one a sign-in page can open on.
    #[error("that account's address cannot be used to sign in")]
    Address,
    /// The sign-in could not be finished.
    #[error("the account could not be signed in")]
    Failed,
    /// accountd could not be asked.
    #[error("the account service could not be reached")]
    Directory,
}

/// Why a sync switch did not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SwitchError {
    /// syncd has no such account.
    #[error("that account is not one of the sync accounts")]
    UnknownAccount,
    /// The account offers nothing of this kind for syncd to keep.
    #[error("this account has nothing of that kind to keep")]
    NothingToKeep,
    /// The grant could not be recorded or taken back.
    #[error("that could not be saved")]
    Unsaved,
    /// The person did not give the access the switch needs (they closed the page, refused it,
    /// or it could not be asked).
    #[error("the access was not given")]
    NotGiven,
}

/// Why an account was not removed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RemoveError {
    /// syncd has no such account.
    #[error("that account is not one of the sync accounts")]
    Unknown,
    /// Its journals or mirrors could not be deleted; nothing else was removed.
    #[error("its files on this computer could not be deleted: {0}")]
    Files(String),
    /// Its stored credentials could not be deleted.
    #[error("its stored sign-in could not be deleted")]
    Secrets,
}

/// How a "Continue as" ended well.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Continued {
    /// syncd signed the identity in: a new account.
    SignedIn(AccountId),
    /// syncd already had this account; nothing was signed in again.
    AlreadyHere(AccountId),
}

impl Continued {
    /// The account either way.
    pub fn account(&self) -> &AccountId {
        match self {
            Continued::SignedIn(id) | Continued::AlreadyHere(id) => id,
        }
    }
}

/// The folder an account's journals, cache and drive are kept under, and its drive's id.
fn drive_dir(account: &AccountId) -> Option<AccountDir> {
    AccountDir::parse(&porter_ids::object_segment(account.as_str()))
}

/// syncd's accounts over the service it hosts and the directory of identities.
#[derive(Debug)]
pub struct Keep<H, D> {
    hosted: std::sync::Arc<H>,
    directory: D,
    intent: Intent,
    /// One switch at a time: the intent names the account being turned on.
    turning: tokio::sync::Mutex<()>,
    paths: Paths,
    hub: Hub,
    /// The drives shown in Files, let go before their account is removed.
    drives: DriveSet,
}

impl<H: Hosted, D: Directory> Keep<H, D> {
    /// The accounts of `hosted`, continuing as the identities `directory` offers. `intent` is
    /// the one the service's [`SyncSheets`] reads; `paths` and `hub` are what a removal wipes.
    pub fn new(
        hosted: std::sync::Arc<H>,
        directory: D,
        intent: Intent,
        paths: Paths,
        hub: Hub,
    ) -> Self {
        Self {
            hosted,
            directory,
            intent,
            turning: tokio::sync::Mutex::new(()),
            paths,
            hub,
            drives: DriveSet::default(),
        }
    }

    /// With the drives shown in Files: a removal lets the account's drive go first, keeping
    /// what was not uploaded in its folder.
    #[must_use]
    pub fn with_drives(mut self, drives: DriveSet) -> Self {
        self.drives = drives;
        self
    }

    /// The registry: the accounts syncd signed in and its own grants.
    pub fn registry(&self) -> porter_service_core::registry::Registry {
        self.hosted.registry()
    }

    /// "Continue as": the person picks an identity, syncd signs it in (unless it already holds
    /// it) and says it uses the identity.
    pub async fn continue_as(&self, window: &ParentWindow) -> Result<Continued, ContinueError> {
        let pick = self
            .directory
            .offer(window)
            .await
            .map_err(|why| match why {
                IdentitiesError::Cancelled => ContinueError::Cancelled,
                IdentitiesError::NoIdentity => ContinueError::NoIdentity,
                _ => ContinueError::Directory,
            })?;
        let held = self
            .hosted
            .registry()
            .accounts
            .into_iter()
            .find(|a| a.label.0 == pick.address.0 && a.provider == pick.provider);
        let continued = match held {
            Some(account) => Continued::AlreadyHere(account.id),
            None => {
                let hint = LoginHint::parse(&pick.address.0).map_err(|_| ContinueError::Address)?;
                // The identity was signed in to accountd with this same client, so the person
                // agreed to what it asks before; the service asks again if the issuer says not.
                let request =
                    SignInAs::new(pick.provider.clone(), hint, ConsentHistory::SignedInBefore);
                match self.hosted.sign_in_as(request, window.clone()).await {
                    AccountsReply::Added(id) => Continued::SignedIn(id),
                    AccountsReply::AlreadyAdded(id) => Continued::AlreadyHere(id),
                    AccountsReply::Refused(Refusal::Dismissed) => {
                        return Err(ContinueError::Cancelled);
                    }
                    _ => return Err(ContinueError::Failed),
                }
            }
        };
        self.directory
            .register_use(&pick.id)
            .await
            .map_err(|_| ContinueError::Directory)?;
        Ok(continued)
    }

    /// "Continue as", then the first thing the account offers for syncd to keep switched on
    /// (the files, else the photos): the person asked to sync this account. The switch turns on
    /// only once the sign-in finished.
    pub async fn continue_and_keep(
        &self,
        window: &ParentWindow,
    ) -> Result<Continued, ContinueError> {
        let continued = self.continue_as(window).await?;
        let account = continued.account().clone();
        let first = crate::keep::account_of(&self.registry(), &account)
            .and_then(|row| sync_offers(&row).first().copied());
        if let Some(class) = first {
            // A switch that is refused leaves the account signed in with its switches off, as
            // the person can turn them on in the rows.
            if let Err(why) = self.set_sync(&account, class, Toggle::On).await {
                eprintln!("syncd: could not switch sync on for a new account: {why}");
            }
        }
        Ok(continued)
    }

    /// Turns syncd's keeping of `class` of `account` on or off.
    pub async fn set_sync(
        &self,
        account: &AccountId,
        class: SyncClass,
        toggle: Toggle,
    ) -> Result<(), SwitchError> {
        let registry = self.hosted.registry();
        let row = account_of(&registry, account).ok_or(SwitchError::UnknownAccount)?;
        match toggle {
            Toggle::On => {
                if !sync_offers(&row).contains(&class) {
                    return Err(SwitchError::NothingToKeep);
                }
                if sync_allowed(&registry.grants, account, class) {
                    return Ok(());
                }
                let _one_switch = self.turning.lock().await;
                self.intent.set(account.clone());
                let reply = self
                    .hosted
                    .answer(AccountsRequest::Choose {
                        need: need_for(&row, class),
                        class: class.class(),
                        usage: Usage::Background,
                        window: ParentWindow::Unparented,
                    })
                    .await;
                self.intent.clear();
                match reply {
                    AccountsReply::Chosen(chosen) if chosen.account == *account => Ok(()),
                    _ => Err(SwitchError::Unsaved),
                }
            }
            _ => self.revoke(&keys_of(account, class)).await,
        }
    }

    /// Whether Files offers "Show in Files" for `account`: its drive is one the listing reads
    /// whole, and its provider asks the whole drive on use.
    pub fn offers_files(&self, account: &Account) -> bool {
        lists_whole_drive(account)
            && self
                .hosted
                .asks_on_use(&account.provider, CapabilityKind::Storage)
    }

    /// "Show in Files" on or off for `account`. On, when the account was not given the whole
    /// drive yet, asks the person for it in a sign-in of its own (the browser asks that alone),
    /// then records syncd's grant of it; a refusal there is [`SwitchError::NotGiven`] and nothing
    /// is recorded. Off takes the grant back; the listing's journal and the access given stay.
    pub async fn show_in_files(
        &self,
        account: &AccountId,
        toggle: Toggle,
    ) -> Result<(), SwitchError> {
        let registry = self.hosted.registry();
        let row = account_of(&registry, account).ok_or(SwitchError::UnknownAccount)?;
        match toggle {
            Toggle::On => {
                if !self.offers_files(&row) {
                    return Err(SwitchError::NothingToKeep);
                }
                if drive_allowed(&registry.grants, account) {
                    return Ok(());
                }
                if !claims_whole_drive(&row) {
                    self.hosted
                        .extend_access(
                            account,
                            vec![CapabilityKind::Storage],
                            ParentWindow::Unparented,
                        )
                        .await
                        .map_err(|why| {
                            eprintln!("syncd: the whole drive was not given: {why:?}");
                            SwitchError::NotGiven
                        })?;
                }
                let _one_switch = self.turning.lock().await;
                self.intent.set(account.clone());
                let reply = self
                    .hosted
                    .answer(AccountsRequest::Choose {
                        need: crate::datasets::storage::drive_need(),
                        class: DataClass::Files,
                        usage: Usage::Background,
                        window: ParentWindow::Unparented,
                    })
                    .await;
                self.intent.clear();
                match reply {
                    AccountsReply::Chosen(chosen) if chosen.account == *account => Ok(()),
                    _ => Err(SwitchError::Unsaved),
                }
            }
            _ => self.revoke(&[drive_key(account)]).await,
        }
    }

    /// "Keep Everything Downloaded" on or off for the drive of `account`, which must be shown in
    /// Files. It is kept in syncd's settings, so the drive keeps everything again after a restart
    /// (and after Show in Files is turned off and on again); the drive shown now follows at once.
    pub fn keep_everything(&self, account: &AccountId, toggle: Toggle) -> Result<(), SwitchError> {
        let registry = self.hosted.registry();
        account_of(&registry, account).ok_or(SwitchError::UnknownAccount)?;
        if !drive_allowed(&registry.grants, account) {
            return Err(SwitchError::NothingToKeep);
        }
        let dir = drive_dir(account).ok_or(SwitchError::UnknownAccount)?;
        self.drives
            .set_keep_all(dir.as_str(), toggle == Toggle::On)
            .map_err(|why| {
                eprintln!("syncd: whether to keep everything downloaded could not be saved: {why}");
                SwitchError::Unsaved
            })
    }

    /// Whether the drive of `account` is kept downloaded whole.
    pub fn keeps_everything(&self, account: &AccountId) -> bool {
        drive_dir(account).is_some_and(|dir| self.drives.keeps_all(dir.as_str()))
    }

    /// Whether keeping the drive of `account` downloaded whole needs more room than this computer
    /// has free.
    pub fn short_of_room(&self, account: &AccountId) -> bool {
        drive_dir(account).is_some_and(|dir| self.drives.short_of_room(dir.as_str()))
    }

    /// Hears the drives whose room to keep everything downloaded changes, by drive id
    /// ([`Keep::account_of_drive`] names the account).
    pub fn watch_room(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.drives.watch_room()
    }

    /// The account whose drive is `drive` (a drive's id is its account's folder name).
    pub fn account_of_drive(&self, drive: &str) -> Option<AccountId> {
        self.hosted
            .registry()
            .accounts
            .into_iter()
            .map(|account| account.id)
            .find(|id| drive_dir(id).is_some_and(|dir| dir.as_str() == drive))
    }

    /// Takes back every grant of syncd's under one of `wanted`.
    async fn revoke(&self, wanted: &[GrantKey]) -> Result<(), SwitchError> {
        let grants = self.hosted.grants();
        for grant in grants.iter().filter(|g| wanted.contains(&g.key)) {
            if self.hosted.revoke(&grant.id).await.is_err() {
                return Err(SwitchError::Unsaved);
            }
        }
        Ok(())
    }

    /// Signs `account` in again, through the sheet.
    pub async fn sign_in_again(
        &self,
        account: &AccountId,
        window: ParentWindow,
    ) -> Result<(), ContinueError> {
        match self.hosted.sign_in_again(account, window).await {
            AccountsReply::Reauthenticated => Ok(()),
            AccountsReply::Refused(Refusal::Dismissed) => Err(ContinueError::Cancelled),
            _ => Err(ContinueError::Failed),
        }
    }

    /// Removes `account`: its drive is let go (held, and what was made or changed in its folder
    /// and not uploaded written out to a folder of its own beside it), then its datasets stop,
    /// its journals, mirrors and cache are deleted, and its stored sign-in and grants. What was
    /// kept. A removal that fails leaves the account and starts its drive again; what was kept
    /// stays where it is, so trying again keeps nothing twice.
    pub async fn remove(&self, account: &AccountId) -> Result<Vec<Kept>, RemoveError> {
        account_of(&self.hosted.registry(), account).ok_or(RemoveError::Unknown)?;
        let dir = drive_dir(account).ok_or(RemoveError::Unknown)?;
        let failed = |why: RemoveError| {
            self.drives.release(dir.as_str());
            why
        };
        let kept = self
            .drives
            .retire(&self.paths, &dir)
            .await
            .map_err(|why| failed(RemoveError::Files(why)))?;
        removal::wipe(&self.paths, &self.hub, &dir)
            .await
            .map_err(|why| failed(RemoveError::Files(why.to_string())))?;
        self.hosted
            .remove_account(account)
            .await
            .map_err(|_| failed(RemoveError::Secrets))?;
        self.drives.forget(dir.as_str());
        Ok(kept)
    }
}
