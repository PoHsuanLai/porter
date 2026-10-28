//! The rows every account has in a daemon's settings module (PLAN §2.5), once. accountd lists
//! the Internet accounts and inferd the AI accounts and assistants; each account has a label,
//! a place, a state, a way it signs in, a provider and a group (read-outs) and the actions
//! "Sign in again" and "Remove account". A row is an [`AccountRow`]: its suffix under
//! `accounts.<id>.`, the [`KeySpec`] that lists it (or none, when the account has no such row)
//! and the value a read answers. The daemons add the rows only they have (accountd's expiry,
//! apps and sign-in keys; inferd's services, permits and spend) beside these, and do the actions
//! (`reauth`, `remove`), which are theirs to carry out.
//!
//! Every key is under `accounts.`, so it is never agent-settable (`ds-settings` refuses a schema
//! that says otherwise).

pub mod accounts;
pub mod clients;
mod helpers;
mod rows;

pub use helpers::{action, labelled, off, readout, spec};
pub use rows::{
    ACTIONS, GROUP, LABEL, PLACE, PROVIDER, READOUTS, REAUTH, REMOVE, ROWS, SIGN_IN, STATE,
    place_slug,
};

use ds_settings::schema::KeySpec;
use porter_core::{Account, AccountId};
use porter_sheet::ProviderGroup;

/// What a row needs to know of an account beyond the account: the words a person reads for its
/// provider, and the part of the page it is listed under.
#[derive(Debug, Clone, Copy)]
pub struct Names<'a> {
    /// What the account's provider is called.
    pub provider_label: &'a str,
    /// The group of the page the account is listed under.
    pub group: ProviderGroup,
}

/// One row every account has.
pub trait AccountRow: Sync + std::fmt::Debug {
    /// The row's path under `accounts.<id>.`.
    fn suffix(&self) -> &'static str;

    /// The key that lists this row for `account` in `section`, or `None` when the account has no
    /// such row (a program on this computer has nothing to sign in to).
    fn spec(&self, account: &Account, section: &str, names: &Names<'_>) -> Option<KeySpec>;

    /// The value a read of the row answers: the text of a read-out, `false` for an action.
    fn read(&self, account: &Account, names: &Names<'_>) -> toml::Value;
}

/// A row, by identity: comparable and printable, so a key that holds one can be too.
#[derive(Clone, Copy)]
pub struct Row(pub &'static dyn AccountRow);

impl Row {
    /// The row named by `suffix`, among [`ROWS`].
    pub fn named(suffix: &str) -> Option<Row> {
        ROWS.iter().copied().find(|row| row.0.suffix() == suffix)
    }

    /// The row's path under `accounts.<id>.`.
    pub fn suffix(self) -> &'static str {
        self.0.suffix()
    }

    /// The row's path for the account `id`.
    pub fn path(self, id: &AccountId) -> String {
        format!("accounts.{id}.{}", self.0.suffix())
    }
}

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
