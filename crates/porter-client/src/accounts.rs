//! `Accounts`: signing an account in, through the sheet.

use crate::changes::Changes;
use crate::error::LinkError;
use crate::link::{AccountWatch, AccountsLink, ServerEdits};
use porter_core::{AccountId, AccountLabel, AccountState, ProviderHint, ProviderId, ReauthReason};
use porter_ids::object_segment;
use porter_sheet::{EditServers, EditedServers, ServersFixed};
use porter_wire::names::ACCOUNTS_PATH;
use porter_wire::{ParentWindow, Refusal};
use std::sync::Arc;

/// Why an accounts call failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AccountsError {
    /// accountd refused, or the person closed the sheet (`Refusal::Dismissed`).
    #[error("refused")]
    Refused(Refusal),
    /// `add`: the person signed in to an account that was already here, so nothing was added. It
    /// is that account; ask for a grant of it.
    #[error("already added")]
    AlreadyAdded(AccountId),
    /// This app has a sheet of this kind open already.
    #[error("a sheet is open")]
    Busy,
    /// The call did not complete.
    #[error(transparent)]
    Link(#[from] LinkError),
}

/// Why editing an account's servers gave nothing back. More reasons may be added: match with a
/// wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EditError {
    /// The provider's sign-in is not a typed password, so its servers are not the person's to
    /// name: show them read-only.
    #[error("fixed")]
    Fixed(ServersFixed),
    /// The service holds the account under another provider than the edit names.
    #[error("not that provider")]
    NotThatProvider,
    /// The sheet ended without servers (the person closed it), or the service refused.
    #[error("refused")]
    Refused(Refusal),
    /// The call did not complete.
    #[error(transparent)]
    Link(#[from] LinkError),
}

/// The accounts domain over a transport.
#[derive(Debug)]
pub struct Accounts<T> {
    pub(crate) link: Arc<T>,
}

impl<T> Clone for Accounts<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: AccountsLink> Accounts<T> {
    /// Opens the add-account sheet; the account added. An account the person signs in to that was
    /// already here is [`AccountsError::AlreadyAdded`] with its id: nothing was added, and the app
    /// may ask for a grant of it.
    pub async fn add(
        &self,
        hint: ProviderHint,
        window: &ParentWindow,
    ) -> Result<AccountId, AccountsError> {
        self.link.add(&hint, window).await
    }

    /// Opens the sheet that signs a granted account in again (its refresh token was revoked, a
    /// password changed). An account the app holds no grant for is not one it can see.
    pub async fn reauthenticate(
        &self,
        account: &AccountId,
        window: &ParentWindow,
    ) -> Result<(), AccountsError> {
        self.link.reauthenticate(account, window).await
    }
}

impl<T: ServerEdits> Accounts<T> {
    /// Opens the server form of an existing password account, prefilled from `edit`, and tries
    /// what the person types at the typed hosts: the incoming server, and the outgoing one when
    /// it, the login name or the password changed. Nothing is stored: the app keeps the
    /// servers, the name and the new password the [`EditedServers`] carries (`into_parts`). Ask
    /// [`porter_sheet::servers_editable`] first to decide whether to offer the edit at all.
    pub async fn edit_servers(
        &self,
        edit: EditServers,
        window: &ParentWindow,
    ) -> Result<EditedServers, EditError> {
        self.link.edit_servers(edit, window).await
    }
}

impl<T: AccountWatch> Accounts<T> {
    /// The accounts that need signing in again now, with their labels. Asking is also how
    /// accountd learns this connection, which it then sends every account's `State` changes.
    pub async fn needing_reauth(&self) -> Result<Vec<NeedsReauth>, LinkError> {
        self.link.needing_reauth().await
    }

    /// One account as accountd shows it to the shell.
    pub async fn view(&self, account: &AccountRef) -> Result<AccountView, LinkError> {
        self.link.view(account).await
    }

    /// Opens the sheet that signs `account` in again, for the shell, which knows an account by
    /// its object only.
    pub async fn reauthenticate_shown(
        &self,
        account: &AccountRef,
        window: &ParentWindow,
    ) -> Result<(), AccountsError> {
        self.link.reauthenticate_shown(account, window).await
    }

    /// The accounts added and removed, and every change of an account's state (an account that
    /// needs signing in again is one such change, and so is its recovery), from now on. Subscribe
    /// before reading [`Accounts::needing_reauth`], so no change falls between the two.
    pub async fn watch(&self) -> Result<Changes<AccountChange>, LinkError> {
        self.link.watch().await
    }
}

/// The object accountd serves an account at, as a signal names it. A lossy spelling of the id
/// (a legal object-path segment): good as a directory name and for asking [`Accounts::view`], not
/// to be turned back into an [`AccountId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountRef {
    segment: String,
}

impl AccountRef {
    /// The account the object path `/org/quire/Accounts1/account/<segment>` names; `None` for any
    /// other path.
    pub fn of_object_path(path: &str) -> Option<Self> {
        let prefix = format!("{ACCOUNTS_PATH}/account/");
        let segment = path.strip_prefix(&prefix)?;
        (!segment.is_empty() && !segment.contains('/')).then(|| Self {
            segment: segment.to_owned(),
        })
    }

    /// The object `account` is served at.
    pub fn of_account(account: &AccountId) -> Self {
        Self {
            segment: object_segment(account.as_str()),
        }
    }

    /// The segment: what names the account's directories (`a1`, `67e55044_10b1_x`).
    pub fn segment(&self) -> &str {
        &self.segment
    }
}

/// An account that needs signing in again.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NeedsReauth {
    /// The account.
    pub account: AccountRef,
    /// What accountd calls it.
    pub label: AccountLabel,
}

impl NeedsReauth {
    /// `account`, called `label`.
    pub fn new(account: AccountRef, label: AccountLabel) -> Self {
        Self { account, label }
    }
}

/// One account as accountd shows it to the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AccountView {
    /// Its id.
    pub id: AccountId,
    /// Its provider.
    pub provider: ProviderId,
    /// What accountd calls it.
    pub label: AccountLabel,
    /// Where it stands.
    pub state: AccountState,
    /// Why it has to be signed in again, when porter knows beyond "the provider refused".
    pub reauth_reason: Option<ReauthReason>,
}

impl AccountView {
    /// An account with these properties.
    pub fn new(
        id: AccountId,
        provider: ProviderId,
        label: AccountLabel,
        state: AccountState,
        reauth_reason: Option<ReauthReason>,
    ) -> Self {
        Self {
            id,
            provider,
            label,
            state,
            reauth_reason,
        }
    }
}

/// What happened to an account, as accountd tells the shell. More may be added: match with a
/// wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccountChange {
    /// An account was added.
    Added(AccountRef),
    /// An account was removed.
    Removed(AccountRef),
    /// An account is in this state now, whichever it came from: signing in again is the state
    /// `AccountState::NeedsReauth`, and an account that recovers is `AccountState::Ok`, so a
    /// shell follows one path for both.
    State(AccountRef, AccountState),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_the_last_segment_of_its_object_path_and_nothing_else_is_one() {
        let path = "/org/quire/Accounts1/account/abc_1";
        assert_eq!(
            AccountRef::of_object_path(path).map(|a| a.segment().to_owned()),
            Some("abc_1".to_owned())
        );
        for other in [
            "/org/quire/Accounts1/account/",
            "/org/quire/Accounts1/account/a/b",
            "/other/abc",
        ] {
            assert_eq!(AccountRef::of_object_path(other), None, "{other}");
        }
    }

    #[test]
    fn an_account_id_is_spelled_as_the_object_accountd_serves_it_at() {
        let id = AccountId::parse("67e55044-10b1.x").expect("id");
        assert_eq!(AccountRef::of_account(&id).segment(), "67e55044_10b1_x");
    }
}
