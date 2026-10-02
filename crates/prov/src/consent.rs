//! The witness that a person said yes, and the two functions that spend it.

use crate::label::{Confidentiality, Labelled};
use porter_core::{CoreError, UnixSeconds, is_id};
use serde::{Deserialize, Serialize};
use std::fmt;

/// One confirmation, as the sheet minted it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConfirmId(String);

impl ConfirmId {
    /// The id written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        if is_id(text) {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "confirm id",
                text: text.to_owned(),
            })
        }
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ConfirmId {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<ConfirmId> for String {
    fn from(id: ConfirmId) -> String {
        id.0
    }
}

impl fmt::Display for ConfirmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the answer came from. Voice is never one of these: a spoken answer cannot confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputProof {
    /// A hardware key or pointer event on the sheet, attested by the compositor.
    HardwareSeat,
    /// The sheet's own pointer event with no compositor attestation (stock compositor).
    SheetFallback,
    /// The shell's own UI called in (memory Keep from sill).
    ShellCaller,
}

/// The record that a person confirmed something.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConfirmReceipt {
    /// Which confirmation.
    pub id: ConfirmId,
    /// What the answer came from.
    pub input: InputProof,
    /// When.
    pub at: UnixSeconds,
}

/// Proof needed to raise integrity or lower confidentiality.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Witness {
    /// The person confirmed it.
    UserConfirmed(ConfirmReceipt),
}

/// Raises integrity to `Trusted` and adds `Source::User`.
pub fn endorse<T>(value: Labelled<T>, witness: &Witness) -> Labelled<T> {
    let _ = (value, witness);
    todo!("integrity Trusted, sources += User")
}

/// Lowers confidentiality to `to`, which must not exceed what the receipt covers.
pub fn declassify<T>(value: Labelled<T>, to: Confidentiality, witness: &Witness) -> Labelled<T> {
    let _ = (value, to, witness);
    todo!("confidentiality := to")
}
