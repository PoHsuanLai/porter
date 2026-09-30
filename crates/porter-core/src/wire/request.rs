//! What an app asks accountd. The caller's identity is not part of it: each transport derives
//! it from the connection.

use crate::consent::Usage;
use crate::data_class::DataClass;
use crate::id::{AccountId, GrantId, ProviderId};
use crate::need::Need;
use crate::token::Audience;
use serde::{Deserialize, Serialize};

/// One request to accountd (design/31 §4.4; the D-Bus methods carry the same values).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum AccountsRequest {
    /// The granted accounts that meet a need (`Manager.Query`).
    Query {
        /// The need.
        need: Need,
        /// The data it touches.
        class: DataClass,
        /// Interactive or background.
        usage: Usage,
    },
    /// Whether a need can be met, revealing no identity (`Manager.Availability`).
    Availability {
        /// The need.
        need: Need,
        /// The data it touches.
        class: DataClass,
        /// Interactive or background.
        usage: Usage,
    },
    /// Ask the user to pick a fitting account and consent (`Manager.Choose`).
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
    /// The caller's own grants (`Grants.List`).
    ListGrants,
    /// Withdraw one of the caller's grants (`Grants.Revoke`).
    Revoke {
        /// The grant.
        grant: GrantId,
    },
    /// A short-lived token for a granted account (`Tokens.IssueToken`).
    IssueToken {
        /// The grant.
        grant: GrantId,
        /// The service it is for.
        audience: Audience,
    },
}

/// The window a sheet attaches to: an xdg-foreign handle as portals take it
/// (`wayland:<handle>`, `x11:<xid>`), or none.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ParentWindow {
    /// No parent: the sheet is a free-standing window.
    Unparented,
    /// The handle.
    Handle(String),
}

/// Which provider the add sheet opens on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ProviderHint {
    /// The provider list.
    Any,
    /// This provider's form.
    Provider(ProviderId),
}
