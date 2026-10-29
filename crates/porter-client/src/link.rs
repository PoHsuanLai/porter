//! What a transport carries, one trait per domain. A transport implements the domains it can
//! serve; a method a transport cannot serve is not on its type, because [`crate::Porter`] hands out
//! a domain client only over a transport that implements the domain's trait.

use crate::accounts::{
    AccountChange, AccountRef, AccountView, AccountsError, EditError, NeedsReauth,
};
use crate::ai_accounts::AiAccountsError;
use crate::ai_keys::AiKeysError;
use crate::authenticated::AuthenticatedStream;
use crate::changes::Changes;
use crate::error::LinkError;
use crate::found::ConsentOffer;
use crate::grants::GrantsError;
use crate::identities::{IdentitiesError, Picked};
use crate::spaces::SpacesError;
use porter_capability::Claim;
use porter_core::consent::{Availability, Grant, Usage};
use porter_core::{
    AccountId, AccountState, AiAccountRow, AppId, AppName, Candidate, DataClass, DesktopSpace,
    DesktopSpaceRecord, EndpointUrl, GrantId, Identity, IdentityId, LlmVerdict, OwnServerRow,
    ProviderHint, ProviderId, SecretText, SpaceChange, SpaceLook, SpaceName,
};
use porter_match::Need;
use porter_sheet::{EditServers, EditedServers};
use porter_tailscale::Machine;
use porter_wire::ParentWindow;
use std::future::Future;

/// The accounts domain: signing an account in through the sheet.
pub trait AccountsLink: Send + Sync + 'static {
    /// Opens the add-account sheet; the account added.
    fn add(
        &self,
        hint: &ProviderHint,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<AccountId, AccountsError>> + Send;

    /// Opens the sheet that signs a granted account in again.
    fn reauthenticate(
        &self,
        account: &AccountId,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<(), AccountsError>> + Send;
}

/// Hosted only: editing a password account's servers through the sheet. The service checks them
/// before it hands them back; no bus carries it, so it is not on a bus transport's type.
pub trait ServerEdits: AccountsLink {
    /// Opens the server form of `edit`'s account, prefilled, and tries what the person types.
    fn edit_servers(
        &self,
        edit: EditServers,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<EditedServers, EditError>> + Send;
}

/// The grants domain: who may use which account, and the relays a grant opens. No bus carries it.
pub trait GrantsLink: Send + Sync + 'static {
    /// The accounts that fit `need` for this app and that it holds a grant for.
    fn candidates(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> impl Future<Output = Result<Vec<Candidate>, GrantsError>> + Send;

    /// Whether `need` can be met for this app, revealing no identity.
    fn availability(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> impl Future<Output = Result<Availability, GrantsError>> + Send;

    /// The grants this app holds.
    fn grants(&self) -> impl Future<Output = Result<Vec<Grant>, GrantsError>> + Send;

    /// Shows the chooser and consent sheet for `offer`; the account the person picked, granted.
    fn request_grant(
        &self,
        offer: &ConsentOffer,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<Candidate, GrantsError>> + Send;

    /// Withdraws one of this app's own grants.
    fn revoke(&self, grant: &GrantId) -> impl Future<Output = Result<(), GrantsError>> + Send;

    /// A byte stream to `endpoint`, one of the account's servers, authenticated by a relay.
    fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<AuthenticatedStream, GrantsError>> + Send;

    /// A byte stream to `origin`, a host the account's provider file declares for the grant's kind
    /// as one its pre-authenticated links point at; the relay adds no credential.
    fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<AuthenticatedStream, GrantsError>> + Send;
}

/// What the shell reads of accountd's accounts, beyond signing one in. The bus only.
pub trait AccountWatch: AccountsLink {
    /// The accounts that need signing in again now, with their labels. Asking is also how accountd
    /// learns this connection, which it then sends `NeedsReauth` to.
    fn needing_reauth(&self) -> impl Future<Output = Result<Vec<NeedsReauth>, LinkError>> + Send;

    /// One account as accountd shows it to the shell.
    fn view(
        &self,
        account: &AccountRef,
    ) -> impl Future<Output = Result<AccountView, LinkError>> + Send;

    /// [`AccountsLink::reauthenticate`] for an account the shell knows by its object only.
    fn reauthenticate_shown(
        &self,
        account: &AccountRef,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<(), AccountsError>> + Send;

    /// The accounts added and removed, and every change of an account's state, from now on.
    fn watch(&self) -> impl Future<Output = Result<Changes<AccountChange>, LinkError>> + Send;
}

/// The identities signed in on this machine ("Continue as"). The bus only.
pub trait IdentitiesLink: Send + Sync + 'static {
    /// The identities signed in on this machine, in accountd's order.
    fn list(&self) -> impl Future<Output = Result<Vec<Identity>, IdentitiesError>> + Send;

    /// "Continue as": shows the person the identity picker and waits for the pick.
    fn offer(
        &self,
        hint: &ProviderHint,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<Picked, IdentitiesError>> + Send;

    /// The password of the identity the person just picked, once.
    fn take_password(
        &self,
        id: &IdentityId,
    ) -> impl Future<Output = Result<SecretText, IdentitiesError>> + Send;

    /// Says that this app signed in with `id`.
    fn register_use(
        &self,
        id: &IdentityId,
    ) -> impl Future<Output = Result<(), IdentitiesError>> + Send;

    /// The apps that signed in with `id`. Only Settings and the shell may ask.
    fn uses(
        &self,
        id: &IdentityId,
    ) -> impl Future<Output = Result<Vec<AppName>, IdentitiesError>> + Send;

    /// The identities removed from now on.
    fn watch_removals(
        &self,
    ) -> impl Future<Output = Result<Changes<IdentityId>, IdentitiesError>> + Send;
}

/// The desktop-wide Spaces. The bus only.
pub trait SpacesLink: Send + Sync + 'static {
    /// Every desktop-wide Space, in the order they were made.
    fn list(&self) -> impl Future<Output = Result<Vec<DesktopSpaceRecord>, SpacesError>> + Send;

    /// Makes a Space and answers its id.
    fn create(
        &self,
        name: &SpaceName,
        look: &SpaceLook,
    ) -> impl Future<Output = Result<DesktopSpace, SpacesError>> + Send;

    /// Registers a Space whose id the caller minted (the compositor, per workspace).
    fn register(
        &self,
        id: &DesktopSpace,
        name: &SpaceName,
        look: &SpaceLook,
    ) -> impl Future<Output = Result<(), SpacesError>> + Send;

    /// Renames a Space.
    fn rename(
        &self,
        id: &DesktopSpace,
        name: &SpaceName,
    ) -> impl Future<Output = Result<(), SpacesError>> + Send;

    /// Gives a Space a new look.
    fn set_look(
        &self,
        id: &DesktopSpace,
        look: &SpaceLook,
    ) -> impl Future<Output = Result<(), SpacesError>> + Send;

    /// Removes a Space.
    fn remove(&self, id: &DesktopSpace) -> impl Future<Output = Result<(), SpacesError>> + Send;

    /// The changes to the Spaces, from now on.
    fn watch(
        &self,
    ) -> impl Future<Output = Result<Changes<(DesktopSpace, SpaceChange)>, SpacesError>> + Send;
}

/// The person's computers on their Tailscale network. The bus only.
pub trait TailnetLink: Send + Sync + 'static {
    /// The other computers on the person's network, none for this one, in accountd's order.
    fn machines(&self) -> impl Future<Output = Result<Vec<Machine>, LinkError>> + Send;

    /// The changes to the list, from now on; read the list again on each.
    fn watch(&self) -> impl Future<Output = Result<Changes<()>, LinkError>> + Send;
}

/// accountd's `Keys`: an app's permit to use the person's AI keys. The bus only.
pub trait AiKeysLink: Send + Sync + 'static {
    /// Shows the person accountd's consent sheet for this app's use of one of their AI keys; the
    /// permit it answers is the person's yes, within the scope they chose.
    fn request_key(
        &self,
        class: DataClass,
        usage: Usage,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<GrantId, AiKeysError>> + Send;
}

/// accountd's `Broker`: what the AI broker (inferd) asks of the daemon that keeps the AI keys.
/// The bus only.
pub trait AiAccountsLink: Send + Sync + 'static {
    /// What the permits say for `app` on each account that serves language models.
    fn verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
    ) -> impl Future<Output = Result<Vec<LlmVerdict>, AiAccountsError>> + Send;

    /// The key behind the allow permit `grant`, read once.
    fn key(
        &self,
        grant: &GrantId,
    ) -> impl Future<Output = Result<SecretText, AiAccountsError>> + Send;

    /// The AI accounts, for the rows the broker adds to them.
    fn accounts(&self) -> impl Future<Output = Result<Vec<AiAccountRow>, AiAccountsError>> + Send;

    /// The accounts that are servers of the person's own.
    fn servers(&self) -> impl Future<Output = Result<Vec<OwnServerRow>, AiAccountsError>> + Send;

    /// The key of the own server `account`, for the one request that asks it for its models.
    fn server_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, AiAccountsError>> + Send;

    /// Reports a probed local runtime as the account of `provider`.
    fn report_local(
        &self,
        provider: &ProviderId,
        claims: &[Claim],
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, AiAccountsError>> + Send;

    /// "An AI account, a permit or a switch changed", from now on.
    fn changes(&self) -> impl Future<Output = Result<Changes<()>, AiAccountsError>> + Send;
}
