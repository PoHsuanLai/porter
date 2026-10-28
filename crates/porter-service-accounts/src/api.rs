//! [`AccountsApi`]: what a host asks of the accounts domain.

use crate::Accounts;
use crate::add_flow::SignInAs;
use crate::add_store::AllowFor;
use crate::agent::AgentFault;
use crate::launcher::Launchers;
use crate::local::LocalFault;
use porter_capability::{CapabilityKind, Claim, Toggle};
use porter_core::{
    AccountId, AccountState, AgentState, AppId, ProviderHint, ProviderId, SecretText,
};
use porter_provider::RevokeOutcome;
use porter_secrets::SecretsError;
use porter_service_core::Ports;
use porter_wire::{AccountsReply, ParentWindow, Refusal};
use std::future::Future;

/// The accounts domain: how an account is made, signed in again, asked more of, rediscovered,
/// switched and removed.
pub trait AccountsApi {
    /// Opens the add-account sheet, drives the provider's sign-in through it, discovers, stores
    /// the account and, when the sheet was opened by an app's chooser, its first grant.
    fn add_account(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Opens the add-account sheet as `add_account` does and, when it ends in an account that
    /// meets `ask`, records one grant of it for `caller` in the same step: the sheet's last
    /// button reads "Add, and allow".
    fn add_and_allow(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
        ask: AllowFor,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Opens the add-account sheet for an identity the host already knows ("Continue as"): the
    /// provider's page opens on `identity.hint`, and a sign-in that skipped the consent screen
    /// and came back with nothing to keep is started once more as a first sign-in. The sheet
    /// opens on `identity.provider` with no provider list; no grant is recorded.
    fn sign_in_as(
        &self,
        caller: &AppId,
        identity: SignInAs,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Runs the provider's sign-in again for an account `caller` holds a grant for and files the
    /// new credential under the same id.
    fn reauthenticate(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Runs the sign-in again for any account, with no grant: for the sheet host and Settings.
    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Asks the person, in a sign-in of its own, for the access `account`'s provider asks only
    /// when it is first used (`ask = "on_use"`) for `kinds`: the whole of a drive, when the
    /// person turns on "Show in Files". The sheet opens on the account, the provider's page asks
    /// for that access alone, and the review names it in plain words; on Allow the new
    /// credential replaces the old under the same id, and the account claims the access given
    /// (the provider's on-use rows of `kinds`), so a need of it fits the account from then on:
    /// an account fits such a need only once the person agreed. An answer from anyone but the
    /// account's own login stores nothing. For the host itself (syncd, for its own accounts): no
    /// grant is asked of `caller`, who is named in the audit.
    ///
    /// # Errors
    /// `UnknownGrant` for an account the registry does not hold; `Unavailable` when its provider
    /// asks none of `kinds` on use, or when what came back could not be kept (another person
    /// included); otherwise the sheet's own ending (`Dismissed`, `Denied` when the person
    /// refused the access).
    fn extend_access(
        &self,
        caller: &AppId,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Whether `provider` asks some access of `kind` only when it is first used (`ask =
    /// "on_use"`): what [`AccountsApi::extend_access`] can ask an account of it for, and so
    /// whether a host offers the switch that asks it ("Show in Files").
    fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool;

    /// The API key filed for `account`: the in-process read for a host that runs the agent
    /// launcher (inferd). It never crosses a process. A key that is missing or unreadable (or
    /// filed as another kind of credential) means signing in again (`NeedsReauth`); a store that
    /// is locked or unreachable is `Unavailable`.
    fn api_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send;

    /// Records what the agent says of its own sign-in. Returns whether the account's state
    /// changed (a repeat of the same word changes nothing and writes nothing).
    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send;

    /// Asks for an agent account's login for an app that holds a grant for it.
    fn login_agent<L: Launchers>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Asks for any agent account's login, with no grant: for the sheet host and Settings.
    fn login_agent_any<L: Launchers>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Takes what a probe found: the account of `provider` (its id is the provider's, one per
    /// runtime) is made if it is not there, holds `models` as its claims, and is in `state`.
    ///
    /// An `offline` report with no models keeps the models the account had, so a stopped runtime
    /// still lists what it ran and comes back as it was. An `ok` report replaces them.
    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send;

    /// Removes an account (PLAN §2.9): asks its provider to stop honouring the credential, best
    /// effort, then wipes its secrets, grants, toggles and row, and audits `Removed`. The
    /// provider's answer never stops the wipe. It answers what the provider said, or `None` when
    /// the provider could not be asked (no credential, unreachable, refused).
    fn remove_with_revoke(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<Option<RevokeOutcome>, SecretsError>> + Send;

    /// Asks the provider what the account can do now and records it (the claims, with the
    /// account's toggles applied). A refresh token the provider rotated while it looked is read
    /// back from a session opened afterwards (`renewed`) and stored, so the next sign-in check
    /// does not present a token the issuer has since replaced.
    fn rediscover(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<Vec<Claim>, Refusal>> + Send;

    /// Switches one kind of one account on or off: the toggle row is kept, and the account's
    /// effective capabilities follow (a kind turned off reads `Absent { TurnedOff }`; turned on
    /// again it reads as its provider file declares it until the next discovery refreshes it).
    /// A switch that changes the kind is audited (`ServiceToggled`).
    fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;
}

impl<E: Ports> AccountsApi for Accounts<'_, E> {
    fn add_account(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        Accounts::add_account(self, caller, hint, window)
    }

    fn add_and_allow(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
        ask: AllowFor,
    ) -> impl Future<Output = AccountsReply> + Send {
        Accounts::add_and_allow(self, caller, hint, window, ask)
    }

    fn sign_in_as(
        &self,
        caller: &AppId,
        identity: SignInAs,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        Accounts::sign_in_as(self, caller, identity, window)
    }

    fn reauthenticate(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        Accounts::reauthenticate(self, caller, account, window)
    }

    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        Accounts::reauthenticate_any(self, caller, account, window)
    }

    fn extend_access(
        &self,
        caller: &AppId,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        Accounts::extend_access(self, caller, account, kinds, window)
    }

    fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool {
        Accounts::asks_on_use(self, provider, kind)
    }

    fn api_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send {
        Accounts::api_key(self, account)
    }

    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send {
        Accounts::set_agent_state(self, account, state)
    }

    fn login_agent<L: Launchers>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send {
        Accounts::login_agent(self, caller, account, window, launchers)
    }

    fn login_agent_any<L: Launchers>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send {
        Accounts::login_agent_any(self, caller, account, window, launchers)
    }

    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send {
        Accounts::report_local(self, provider, models, state)
    }

    fn remove_with_revoke(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<Option<RevokeOutcome>, SecretsError>> + Send {
        Accounts::remove_with_revoke(self, id)
    }

    fn rediscover(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<Vec<Claim>, Refusal>> + Send {
        Accounts::rediscover(self, id)
    }

    fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        Accounts::set_toggle(self, id, kind, toggle)
    }
}
