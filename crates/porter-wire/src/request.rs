//! What an app asks accountd. The caller's identity is not part of it: each transport derives
//! it from the connection.

use porter_core::AccountId;
use porter_core::consent::Usage;
use porter_core::{DataClass, ProviderHint};
use porter_match::Need;
use serde::{Deserialize, Serialize};

/// One request to accountd (design/31 §4.4; the D-Bus methods carry the same values).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccountsRequest {
    /// Ask the user to pick a fitting account and consent (`Keys.RequestKey`).
    Choose {
        /// The need.
        need: Need,
        /// The data it touches.
        class: DataClass,
        /// Interactive or background.
        usage: Usage,
        /// The window the sheet attaches to.
        window: ParentWindow,
    },
    /// Open the add-account sheet (`Manager.AddAccount`).
    AddAccount {
        /// A provider to preselect.
        hint: ProviderHint,
        /// The window the sheet attaches to.
        window: ParentWindow,
    },
    /// Sign a granted account in again (`Account.Reauthenticate`).
    Reauthenticate {
        /// The account.
        account: AccountId,
        /// The window the sheet attaches to.
        window: ParentWindow,
    },
}

/// The window a sheet attaches to: an xdg-foreign handle as portals take it
/// (`wayland:<handle>`, `x11:<xid>`), or none.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ParentWindow {
    /// No parent: the sheet is a free-standing window.
    Unparented,
    /// The handle.
    Handle(String),
}
