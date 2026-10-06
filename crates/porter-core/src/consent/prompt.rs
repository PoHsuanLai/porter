//! The consent sheet accounts-ui draws, as values: what it is asked and what it answers.

use super::grant::{GrantScope, Usage};
use crate::account::AccountLabel;
use crate::app_id::AppId;
use crate::capability::CapabilityKind;
use crate::data_class::DataClass;
use crate::id::{AccountId, ProviderId};
use serde::{Deserialize, Serialize};

/// One consent question: "Photos wants to keep its library in your files", with the fitting
/// accounts to choose from (design/31 §4.5: by capability, not by provider).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentAsk {
    /// Who asks.
    pub app: AppId,
    /// For what kind.
    pub kind: CapabilityKind,
    /// Touching which data.
    pub class: DataClass,
    /// Interactive or background use.
    pub usage: Usage,
    /// The accounts that fit, in the order to show them. Never empty: with no fitting account
    /// the sheet offers "Add Account…" instead, which is a separate request.
    pub accounts: Vec<AccountChoice>,
}

/// One account row in the chooser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountChoice {
    /// The account.
    pub account: AccountId,
    /// Its label.
    pub label: AccountLabel,
    /// Its provider, for the mark.
    pub provider: ProviderId,
}

/// What the user answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ConsentAnswer {
    /// "Allow", with the account picked.
    Allow {
        /// The account chosen.
        account: AccountId,
        /// Once or always.
        scope: GrantScope,
    },
    /// "Don't Allow": stored, so the app is not prompted again until Settings changes it.
    Deny,
    /// The sheet was closed without an answer; nothing is stored.
    Dismissed,
    /// "Add Account…": add an account and allow the app in the same step (the add sheet's last
    /// button reads "Add, and allow"). Cancelling the add stores no grant.
    AddAccount,
}
