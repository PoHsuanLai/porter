//! The rows an account has in a daemon's settings module (PLAN §2.5), once. accountd lists
//! the Internet accounts and inferd the AI accounts and assistants; each account has a label,
//! a place, a state, a way it signs in, a provider and a group (read-outs) and the actions
//! "Sign in again" and "Remove account"; an identity has the expiry of a sign-in that expires and
//! the apps using it; an AI account or an assistant has a switch for each service, a permit row
//! group for each permit and, for an assistant, a way to sign out.
//!
//! A row is an [`AccountRow`]: the part of its path after `accounts.<id>.`, the [`KeySpec`]s that
//! list it (none, when the account has no such row), the value a read answers and what a write
//! does. A row whose path carries an id or an index (`used_by.<n>`, `service.<kind>`,
//! `grant.<grant>`) answers every path under its name, so a new row is one impl and one line of
//! [`ROWS`]. The daemon hosts the module and carries out the actions through [`Acts`]; only the
//! spend rows, which read inferd's ledger, are served beside these by inferd.
//!
//! Every key is under `accounts.`, so it is never agent-settable (`ds-settings` refuses a schema
//! that says otherwise).

pub mod accounts;
mod ai_rows;
pub mod clients;
mod helpers;
mod identity_rows;
mod rows;

pub use ai_rows::{APP, GRANT_SERVICE, GrantRow, REVOKE, SCOPE, SINCE};
pub use helpers::{action, labelled, off, readout, spec};
pub use rows::{
    EXPIRES, GROUP, LABEL, PERMITS, PLACE, PROVIDER, REAUTH, REMOVE, ROWS, SERVICE, SIGN_IN,
    SIGN_OUT, STATE, USED_BY, place_slug,
};

use accounts::{Directory, UsedBy};
use ds_settings::live::LiveError;
use ds_settings::schema::KeySpec;
use porter_capability::{CapabilityKind, Toggle};
use porter_core::consent::Grant;
use porter_core::{Account, AccountId, AppName, GrantId};
use porter_service_core::registry::Registry;
use porter_sheet::ProviderGroup;
use std::future::Future;
use std::pin::Pin;

/// What a write is: a future the daemon drives.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a row asks about one account: the account, what the registry holds beside it and how the
/// daemon names providers and apps.
pub struct At<'a> {
    /// The account the rows are of.
    pub account: &'a Account,
    /// Every permit the registry holds. A permit row names its account in its path, but a permit
    /// is found by its id among all of them.
    pub grants: &'a [Grant],
    /// The apps that use the account as an identity, in name order.
    pub used_by: &'a [AppName],
    /// How the daemon calls providers and apps.
    pub directory: &'a dyn Directory,
}

impl std::fmt::Debug for At<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("At")
            .field("account", &self.account.id)
            .finish_non_exhaustive()
    }
}

impl<'a> At<'a> {
    /// The rows' view of `account` in `registry`; `used_by` says which apps use which identity.
    pub fn of(
        account: &'a Account,
        registry: &'a Registry,
        used_by: &'a UsedBy,
        directory: &'a dyn Directory,
    ) -> Self {
        At {
            account,
            grants: &registry.grants,
            used_by: used_by.get(&account.id).map(Vec::as_slice).unwrap_or(&[]),
            directory,
        }
    }
}

impl At<'_> {
    /// The path of the row `tail` of the account.
    pub fn path(&self, tail: &str) -> String {
        format!("accounts.{}.{tail}", self.account.id)
    }

    /// The section of the page the account's rows are in: the account's own label.
    pub fn section(&self) -> &str {
        &self.account.label.0
    }

    /// What a person reads for the account's provider: its file's name, else its id.
    pub fn provider_label(&self) -> &str {
        self.directory
            .provider_label(&self.account.provider)
            .unwrap_or_else(|| self.account.provider.as_str())
    }

    /// The group of the page the account is listed under.
    pub fn group(&self) -> ProviderGroup {
        self.directory.group_of(self.account)
    }
}

/// What the daemon does for the action rows. Each is one thing the daemon can be asked to do to an
/// account; which row asks for it is the row's business.
pub trait Acts: Sync {
    /// Takes the account away.
    fn remove<'a>(&'a self, id: &'a AccountId) -> Boxed<'a, Result<(), LiveError>>;
    /// Starts signing the account in again, and tells the row how it ended.
    fn sign_in_again<'a>(&'a self, account: &'a Account) -> Boxed<'a, Result<(), LiveError>>;
    /// Asks an assistant to sign itself out, and tells the row how it ended.
    fn sign_out<'a>(&'a self, id: &'a AccountId) -> Boxed<'a, Result<(), LiveError>>;
    /// Turns a service of the account on or off for apps.
    fn switch<'a>(
        &'a self,
        id: &'a AccountId,
        kind: CapabilityKind,
        to: Toggle,
    ) -> Boxed<'a, Result<(), LiveError>>;
    /// Takes a permit back.
    fn revoke<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<(), LiveError>>;
}

/// One kind of row an account has.
pub trait AccountRow: Sync + std::fmt::Debug {
    /// The row's path under `accounts.<id>.`, or the first part of it when the path carries an id
    /// or an index.
    fn suffix(&self) -> &'static str;

    /// Whether `tail`, a path after `accounts.<id>.`, names a row of this kind of `account`, among
    /// the permits `grants` the registry holds. A plain row is the one path; one with an id or an
    /// index says which ids and indexes it has.
    fn claims(&self, tail: &str, _account: &Account, _grants: &[Grant]) -> bool {
        tail == self.suffix()
    }

    /// The keys that list this row for the account, in the order they are listed: none when the
    /// account has no such row (a program on this computer has nothing to sign in to), one for a
    /// plain row, one for each index or id of the rest.
    fn specs(&self, at: &At<'_>) -> Vec<KeySpec>;

    /// The value a read of `tail` answers (the text of a read-out, `false` for an action), or
    /// `None` when there is no such value.
    fn read(&self, tail: &str, at: &At<'_>) -> Option<toml::Value>;

    /// What a write of `value` to `tail` does. A read-out is not written.
    fn write<'a>(
        &'a self,
        _tail: &'a str,
        _value: toml::Value,
        _at: &'a At<'a>,
        _acts: &'a dyn Acts,
    ) -> Boxed<'a, Result<(), LiveError>> {
        helpers::refused(LiveError::NotPermitted("this row is a read-out".into()))
    }
}

/// A row, by identity: comparable and printable, so a key that holds one can be too.
#[derive(Clone, Copy)]
pub struct Row(pub &'static dyn AccountRow);

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        self.0.suffix() == other.0.suffix()
    }
}

impl Eq for Row {}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Row").field(&self.0.suffix()).finish()
    }
}

/// The text of `value` as the switch of a service says it, `on` or `off`.
pub(crate) fn toggle_of(value: &toml::Value) -> Option<Toggle> {
    match value {
        toml::Value::String(s) if s == "on" => Some(Toggle::On),
        toml::Value::String(s) if s == "off" => Some(Toggle::Off),
        toml::Value::Boolean(true) => Some(Toggle::On),
        toml::Value::Boolean(false) => Some(Toggle::Off),
        _ => None,
    }
}
