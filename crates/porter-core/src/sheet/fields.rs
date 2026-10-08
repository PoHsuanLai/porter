//! The fields a sign-in asks for. A field is a kind, not a label: the UI words it.

use crate::secret::SecretText;
use serde::{Deserialize, Serialize};

/// What a field is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// The account's address (`ada@example.org`).
    Address,
    /// A server the lookup did not find.
    Server,
    /// A user name that is not the address.
    Username,
    /// A password.
    Password,
    /// An app password: one the person makes in their account's settings for this computer,
    /// because the service does not take the account's own password from a mail app (iCloud,
    /// Fastmail, Yahoo: the providers whose sign-in is `app_password`). The UI words it "App
    /// password" and says where the provider makes one (the sheet's provider id says which).
    AppPassword,
    /// An API key.
    ApiKey,
    /// An API token (a JMAP bearer token).
    Token,
    /// How a typed mail server speaks: a choice (`FieldKind::choices`), `imap` or `jmap`.
    Protocol,
    /// The incoming server's port, a number; empty means the port the protocol and security
    /// usually use.
    Port,
    /// The incoming server's security, a choice: `tls` or `starttls`.
    Security,
    /// The outgoing (SMTP) server's host.
    OutgoingServer,
    /// The outgoing server's port, as `Port`.
    OutgoingPort,
    /// The outgoing server's security, a choice: `tls` or `starttls`.
    OutgoingSecurity,
    /// A JMAP session URL (`https://...`).
    SessionUrl,
}

impl FieldKind {
    /// The values a choice field takes, as the answer's text; empty for a field that is typed.
    /// A host words each value and draws the choice (a segmented control, a pop-up); what it
    /// sends back is one of these.
    pub fn choices(self) -> &'static [&'static str] {
        match self {
            FieldKind::Protocol => super::manual::PROTOCOL_CHOICES,
            FieldKind::Security | FieldKind::OutgoingSecurity => super::manual::SECURITY_CHOICES,
            _ => &[],
        }
    }
}

/// Whether what is typed is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entry {
    /// Shown as typed.
    Plain,
    /// Hidden, and carried as secret text.
    Secret,
}

/// Whether the person may leave a field empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// The sign-in cannot go on without it.
    Required,
    /// May stay empty.
    Optional,
}

/// One field a sign-in asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSpec {
    /// What it is for.
    pub kind: FieldKind,
    /// Shown or hidden.
    pub entry: Entry,
    /// Required or optional.
    pub presence: Presence,
    /// What it starts with (the address typed a moment ago); never a secret.
    pub prefill: Option<String>,
}

/// Whether an answer holds nothing but white space.
pub(super) fn is_empty(value: &FieldValue) -> bool {
    match value {
        FieldValue::Plain(text) => text.trim().is_empty(),
        FieldValue::Secret(secret) => secret.expose().is_empty(),
    }
}

/// The first required field with no answer, in the form's order.
pub fn first_missing(fields: &[FieldSpec], answers: &[FieldAnswer]) -> Option<FieldKind> {
    fields
        .iter()
        .filter(|spec| spec.presence == Presence::Required)
        .find(|spec| {
            answers
                .iter()
                .find(|a| a.kind == spec.kind)
                .is_none_or(|a| is_empty(&a.value))
        })
        .map(|spec| spec.kind)
}

/// What was typed. `Debug` redacts the secret form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum FieldValue {
    /// Text that is not a secret.
    Plain(String),
    /// A password, a key or a token.
    Secret(SecretText),
}

/// One field's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldAnswer {
    /// Which field.
    pub kind: FieldKind,
    /// What was typed.
    pub value: FieldValue,
}
