//! What the sheet host draws. One value per sheet state; the host maps it to quire's
//! `ds-shell::accounts` props in one small table-tested file.

use super::fields::{FieldKind, FieldSpec};
use super::group::ProviderGroup;
use super::mark::MarkFace;
use super::progress::{Review, SignInFault, UserCode};
use crate::app_id::{AppId, AppLabel};
use crate::consent::ConsentAsk;
use crate::endpoint::EndpointUrl;
use crate::id::ProviderId;
use crate::weburl::WebUrl;
use serde::{Deserialize, Serialize};

/// One provider in the list the person picks from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRow {
    /// The provider file's id.
    pub id: ProviderId,
    /// What the user reads ("Nextcloud").
    pub label: String,
    /// The provider mark glyph's name.
    pub mark: String,
    /// Whether this is a provider or the generic "Other…" row.
    #[serde(default)]
    pub kind: RowKind,
    /// Whether signing in is porter's or the agent's own, from the provider file's auth kind.
    /// The row's `label` is the display name ("Claude Code").
    #[serde(default)]
    pub auth: ProviderKind,
    /// The letter and colour the provider file gives its mark, for a provider the UI has no
    /// named mark for. Order of use: this face, then the named variant `mark`, then "@". Left
    /// out when the file has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_face: Option<MarkFace>,
    /// The part of the Accounts page the provider's accounts are listed under, and so the part
    /// of the add list it is in. Left out when unknown (a row written by an older build).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<ProviderGroup>,
}

/// Who signs a provider's account in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// A service porter signs the person in to (every provider whose auth kind is not
    /// `agent_login`).
    #[default]
    Service,
    /// An agent program that signs itself in (`agent_login`); porter asks its launcher.
    AgentLogin,
}

/// What a row of the provider list stands for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    /// One named provider.
    #[default]
    Provider,
    /// A generic protocol file (`generic-imap`, `generic-dav`, `generic-jmap`): the "Other…" row.
    Generic,
}

/// What is wrong with a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// A required field was left empty.
    Missing,
    /// The server refused what was typed.
    Refused,
    /// What was typed cannot be right (a port that is not a number, a host with a path, a
    /// session URL that is not `https`); checked before anything is sent.
    Invalid,
}

/// A field to mark, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldProblem {
    /// Which field.
    pub field: FieldKind,
    /// What is wrong.
    pub problem: ProblemKind,
}

/// The sign-in form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInView {
    /// The provider being signed in to.
    pub provider: ProviderId,
    /// The provider's row: display name and kind. `None` only in a view written before
    /// vocabulary 6.
    #[serde(default)]
    pub row: Option<ProviderRow>,
    /// The fields, in order.
    pub fields: Vec<FieldSpec>,
    /// A field to mark after a refusal or an empty submit.
    pub problem: Option<FieldProblem>,
}

/// The review step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewView {
    /// The provider.
    pub provider: ProviderId,
    /// The provider's row (see [`SignInView::row`]).
    #[serde(default)]
    pub row: Option<ProviderRow>,
    /// What was found.
    pub review: Review,
    /// The app whose chooser started this: the final button reads "Add, and allow <app> to use
    /// it", one grant and not a second prompt. Absent when the sheet was opened from Settings.
    pub allow: Option<AppId>,
    /// What a person calls `allow`'s app, as accountd resolves it (see [`ConsentAsk::app_label`]).
    /// Absent when `allow` is, or when accountd has no name for the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_label: Option<AppLabel>,
}

/// What the host shows now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum SheetView {
    /// "App wants to use an account": the chooser and consent alert.
    Consent(ConsentAsk),
    /// The provider list.
    Providers(Vec<ProviderRow>),
    /// The sign-in form.
    SignIn(SignInView),
    /// "Continue in your browser", with "Copy link".
    BrowserWait {
        /// The provider.
        provider: ProviderId,
        /// The provider's row (see [`SignInView::row`]).
        #[serde(default)]
        row: Option<ProviderRow>,
        /// The page that was opened.
        url: WebUrl,
    },
    /// "Enter this code at ...".
    ShowCode {
        /// The provider.
        provider: ProviderId,
        /// The provider's row (see [`SignInView::row`]).
        #[serde(default)]
        row: Option<ProviderRow>,
        /// The code.
        user_code: UserCode,
        /// The page.
        url: EndpointUrl,
    },
    /// The services found, before anything is stored.
    Review(ReviewView),
    /// Something is running and nothing can be pressed.
    Working {
        /// The provider.
        provider: ProviderId,
        /// The provider's row (see [`SignInView::row`]).
        #[serde(default)]
        row: Option<ProviderRow>,
    },
    /// It ended without an account.
    Failed {
        /// The provider.
        provider: ProviderId,
        /// The provider's row (see [`SignInView::row`]).
        #[serde(default)]
        row: Option<ProviderRow>,
        /// Why.
        fault: SignInFault,
    },
    /// The account is added.
    Done,
}
