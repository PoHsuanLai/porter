//! What a sign-in says as it goes, and what it is told: the secret-free half of the provider's
//! `SignInStep`, and its `SignInInput`.

use super::fields::{FieldAnswer, FieldSpec};
use crate::account::AccountLabel;
use crate::capability::CapabilityKind;
use crate::effective::Toggle;
use crate::endpoint::{EndpointUrl, ServiceEndpoint};
use crate::offer::AbsentReason;
use crate::restriction::LimitReason;
use crate::weburl::WebUrl;
use serde::{Deserialize, Serialize};

/// The short code a person types at a provider's page (the device-code flow). Shown, not secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserCode(pub String);

/// Why a sign-in ended without an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInFault {
    /// The provider refused the credential.
    Refused,
    /// The server could not be reached.
    Unreachable,
    /// The server answered something unreadable.
    Unreadable,
    /// An OAuth issuer with no client registered for this build (Settings > Advanced takes one).
    NeedsClientId,
    /// The person did not finish in time (the browser step's limit).
    TimedOut,
    /// The person closed the sheet or the browser step.
    Cancelled,
    /// The organisation or the provider forbids it (a tenant needing admin consent).
    Forbidden,
    /// The sign-in succeeded but the account could not be stored (the disk or the secret store).
    StoreFailed,
    /// An agent's own login was asked for and no launcher is registered for its program.
    NoLauncher,
    /// The launcher was asked and did not answer in time (accountd's own clock).
    Expired,
    /// The agent program is not installed (the launcher's word).
    NotInstalled,
}

/// One service row of the review step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRow {
    /// Which kind.
    pub kind: CapabilityKind,
    /// On or off, or why the provider has none.
    pub state: ServiceState,
    /// What limits it, for the secondary line.
    pub limit: Option<LimitReason>,
}

/// Whether the account will use a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ServiceState {
    /// The provider offers it; the person may switch it.
    Offered(Toggle),
    /// The provider offers none, for this reason.
    Absent(AbsentReason),
}

/// What discovery found, shown before anything is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    /// The name the account will have.
    pub label: AccountLabel,
    /// Its services.
    pub services: Vec<ServiceRow>,
    /// Its servers.
    pub endpoints: Vec<ServiceEndpoint>,
}

/// What a sign-in reports; the provider's `SignInStep` without the credential, which stays in
/// the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Progress {
    /// It needs these answers.
    Ask(Vec<FieldSpec>),
    /// The person goes to this page in the browser.
    Browser(WebUrl),
    /// The person types this code at this page.
    Code {
        /// The code.
        user_code: UserCode,
        /// The page.
        url: EndpointUrl,
    },
    /// Waiting on the browser or the provider; the host asks again.
    Waiting,
    /// It found these; nothing is stored yet.
    Review(Review),
    /// Signed in; the host holds the credential.
    Done,
    /// It ended without an account.
    Failed(SignInFault),
}

/// One of the person's choices on the review step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceChoice {
    /// The kind.
    pub kind: CapabilityKind,
    /// Their choice.
    pub toggle: Toggle,
}

/// What the host tells a sign-in. It may hold secret text, so it is never serialised; its
/// `Debug` redacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInInput {
    /// Begin.
    Start,
    /// The answers to the fields it asked for.
    Fields(Vec<FieldAnswer>),
    /// The person confirmed the review, with their service choices.
    Confirm(Vec<ServiceChoice>),
    /// Carry on waiting.
    Poll,
    /// Stop.
    Cancel,
}
