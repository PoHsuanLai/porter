//! The consent sheet accounts-ui draws, as values: what it is asked and what it answers.

use super::grant::{GrantScope, Usage};
use crate::account::AccountLabel;
use crate::app_id::{AppId, AppLabel};
use crate::capability::CapabilityKind;
use crate::data_class::DataClass;
use crate::launcher_session::LauncherSession;
use crate::{AccountId, ProviderId};
use serde::{Deserialize, Serialize};

/// One consent question: "Photos wants to keep its library in your files", with the fitting
/// accounts to choose from (design/31 §4.5: by capability, not by provider).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// The launcher session the request was made inside: when present the sheet may offer "This
    /// session only" (`GrantScope::Session` with this id) beside Once and Always, and when
    /// absent it must not (an answer with a session scope is then treated as dismissed). Only
    /// `Peer.RequestAgentGrant` sets it; `app` is then the agent's `org.quire.Agent.<program>`,
    /// which the sheet titles itself with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<LauncherSession>,
    /// What a person calls `app` ("Claude Code" for `org.quire.Agent.claude-code`), as accountd
    /// resolves it. Absent when accountd has no name for it: the host then names the app as it
    /// can from the id. Filled by accountd on the way to the sheet host, never by the service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_label: Option<AppLabel>,
    /// What the answer is for. [`AskPurpose::Use`] (the default, and left out of the JSON) is a
    /// consent: the answer becomes a grant. [`AskPurpose::ContinueAs`] is `Identities1.Offer`:
    /// the person picks which identity the app continues as, and nothing is granted. A host that
    /// does not know the field draws the ordinary chooser, and its `Allow` is read the same way.
    #[serde(default, skip_serializing_if = "AskPurpose::is_use")]
    pub purpose: AskPurpose,
}

/// What a [`ConsentAsk`]'s answer is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AskPurpose {
    /// The app wants to use an account: the answer is stored as a grant.
    #[default]
    Use,
    /// The app offers "Continue as": the answer only names the identity, and no grant is made.
    ContinueAs,
}

impl AskPurpose {
    /// Whether this is the ordinary consent (so the JSON leaves it out).
    pub fn is_use(&self) -> bool {
        matches!(self, AskPurpose::Use)
    }
}

impl ConsentAsk {
    /// A question from `app` for this kind and data class, with the accounts that fit; it asks
    /// outside any launcher session and names the app by its id until accountd fills the label.
    pub fn new(
        app: AppId,
        kind: CapabilityKind,
        class: DataClass,
        usage: Usage,
        accounts: Vec<AccountChoice>,
    ) -> Self {
        Self {
            app,
            kind,
            class,
            usage,
            accounts,
            session: None,
            app_label: None,
            purpose: AskPurpose::Use,
        }
    }

    /// The same question, for this purpose.
    #[must_use]
    pub fn with_purpose(mut self, purpose: AskPurpose) -> Self {
        self.purpose = purpose;
        self
    }

    /// The same question, asked inside this launcher session.
    pub fn with_session(mut self, session: LauncherSession) -> Self {
        self.session = Some(session);
        self
    }

    /// The same question, with what a person calls the app.
    pub fn with_app_label(mut self, label: AppLabel) -> Self {
        self.app_label = Some(label);
        self
    }
}

/// One account row in the chooser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AccountChoice {
    /// The account.
    pub account: AccountId,
    /// Its label.
    pub label: AccountLabel,
    /// Its provider, for the mark.
    pub provider: ProviderId,
}

impl AccountChoice {
    /// A row for this account, with its label and provider.
    pub fn new(account: AccountId, label: AccountLabel, provider: ProviderId) -> Self {
        Self {
            account,
            label,
            provider,
        }
    }
}

/// What the user answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConsentAnswer {
    /// "Allow", with the account picked.
    Allow {
        /// The account chosen.
        account: AccountId,
        /// Once, always, or (only when the ask carried a session) that session.
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
