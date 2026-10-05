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
    /// A password or app password.
    Password,
    /// An API key.
    ApiKey,
    /// An API token (a JMAP bearer token).
    Token,
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
