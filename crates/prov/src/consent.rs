//! The witness that a person said yes, and the two functions that spend it.

use crate::label::{Confidentiality, Integrity, Labelled, Source};
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
    /// The most open label the person agreed the confirmed content may take: what the sheet
    /// showed it going to. [`declassify`] never lowers below it. `Secret` for a confirmation
    /// that opens nothing (an action's, an endorsement's).
    pub covers: Confidentiality,
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
    let Witness::UserConfirmed(_) = witness;
    let mut label = value.label;
    label.integrity = Integrity::Trusted;
    label.sources.insert(Source::User);
    Labelled { label, ..value }
}

/// Why [`declassify`] refused: the label asked for is more open than the receipt covers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the receipt covers {covers:?} and no more openly")]
pub struct BeyondReceipt {
    /// What the receipt covers.
    pub covers: Confidentiality,
}

/// Lowers confidentiality to `to`, which must be at least as restrictive as what the receipt
/// covers: a person who confirmed a release to one Space has not confirmed `Public`.
pub fn declassify<T>(
    value: Labelled<T>,
    to: Confidentiality,
    witness: &Witness,
) -> Result<Labelled<T>, BeyondReceipt> {
    let Witness::UserConfirmed(receipt) = witness;
    // `a.join(b) == b` says `b` is at least as restrictive as `a`; joining `Public` normalises.
    let within = receipt.covers.join(&to) == to.join(&Confidentiality::Public);
    match within {
        true => {
            let mut label = value.label;
            label.confidentiality = to;
            Ok(Labelled { label, ..value })
        }
        false => Err(BeyondReceipt {
            covers: receipt.covers.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::Label;
    use porter_core::SpaceId;

    fn private(spaces: &[&str]) -> Confidentiality {
        Confidentiality::Private(
            spaces
                .iter()
                .map(|s| SpaceId::parse(s).expect("space"))
                .collect(),
        )
    }

    fn witness(covers: Confidentiality) -> Witness {
        Witness::UserConfirmed(ConfirmReceipt {
            id: ConfirmId::parse("c-1").expect("id"),
            input: InputProof::HardwareSeat,
            at: UnixSeconds(1),
            covers,
        })
    }

    fn secret_note() -> Labelled<&'static str> {
        Labelled {
            value: "n",
            label: Label {
                confidentiality: Confidentiality::Secret,
                ..Label::trusted_user()
            },
        }
    }

    #[test]
    fn a_receipt_covers_its_label_and_everything_more_restrictive() {
        use Confidentiality::{Public, Secret};
        let cases = [
            ("public covers all: public", Public, Public, true),
            (
                "public covers all: a space",
                Public,
                private(&["home"]),
                true,
            ),
            ("public covers all: secret", Public, Secret, true),
            (
                "a space does not cover public",
                private(&["home"]),
                Public,
                false,
            ),
            (
                "a space covers itself",
                private(&["home"]),
                private(&["home"]),
                true,
            ),
            (
                "a space covers a wider set",
                private(&["home"]),
                private(&["home", "work"]),
                true,
            ),
            (
                "a space does not cover another",
                private(&["home"]),
                private(&["work"]),
                false,
            ),
            ("a space covers secret", private(&["home"]), Secret, true),
            ("secret covers only secret", Secret, Secret, true),
            (
                "secret does not cover a space",
                Secret,
                private(&["home"]),
                false,
            ),
            ("secret does not cover public", Secret, Public, false),
            (
                "desktop is inside every space",
                private(&["desktop"]),
                private(&["work"]),
                true,
            ),
            (
                "a space does not cover desktop",
                private(&["work"]),
                private(&["desktop"]),
                false,
            ),
        ];
        for (name, covers, to, allowed) in cases {
            let got = declassify(secret_note(), to.clone(), &witness(covers.clone()));
            match (allowed, got) {
                (true, Ok(value)) => assert_eq!(value.label.confidentiality, to, "{name}"),
                (false, Err(error)) => assert_eq!(error.covers, covers, "{name}"),
                (allowed, got) => panic!("{name}: allowed {allowed}, got {got:?}"),
            }
        }
    }

    #[test]
    fn a_refused_declassify_changes_nothing_and_endorse_ignores_the_scope() {
        let note = secret_note();
        let refused = declassify(
            note.clone(),
            Confidentiality::Public,
            &witness(Confidentiality::Secret),
        );
        assert!(refused.is_err());
        let endorsed = endorse(note, &witness(Confidentiality::Secret));
        assert_eq!(endorsed.label.confidentiality, Confidentiality::Secret);
        assert_eq!(endorsed.label.integrity, Integrity::Trusted);
    }
}
