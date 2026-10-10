//! Where the assistant may run: the ids of the places a person lets it use.
//!
//! A place is this computer, one of the person's own computers, or a signed-in cloud account.
//! docket lets the person choose which are allowed and sends inferd the allowed set; inferd's
//! `Places` lists the ones known now. The id is one string, in three text forms:
//!
//! - `this-computer`;
//! - `computer:<name>`, a machine from inferd's attached engines (`where = "my-network"`), named
//!   by the engine's `computer` key;
//! - `account:<account id>`, a cloud AI account accountd holds.

use porter_core::{AccountId, CoreError, is_id};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The text of the place that is this computer.
const THIS_COMPUTER: &str = "this-computer";
/// The prefix of an own computer's id.
const COMPUTER_PREFIX: &str = "computer:";
/// The prefix of a cloud account's id.
const ACCOUNT_PREFIX: &str = "account:";

/// What a place is, without saying which: the closed set a person chooses by, and what
/// `would_need` of a refusal names. The slugs are the serde form and the bus's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    /// The computer the assistant runs on.
    ThisComputer,
    /// Another computer of the person's own.
    OwnComputer,
    /// A signed-in cloud account.
    CloudAccount,
}

impl PlaceKind {
    /// The kind's slug.
    pub fn slug(self) -> &'static str {
        match self {
            PlaceKind::ThisComputer => "this_computer",
            PlaceKind::OwnComputer => "own_computer",
            PlaceKind::CloudAccount => "cloud_account",
        }
    }

    /// The kind a slug names.
    pub fn from_slug(slug: &str) -> Option<Self> {
        [
            PlaceKind::ThisComputer,
            PlaceKind::OwnComputer,
            PlaceKind::CloudAccount,
        ]
        .into_iter()
        .find(|kind| kind.slug() == slug)
    }
}

/// The name of one of the person's own computers: a lowercase slug (`lab`, `studio-2`), the same
/// grammar as every id of porter's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ComputerName(String);

impl ComputerName {
    /// The name written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        if is_id(text) {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "computer name",
                text: text.to_owned(),
            })
        }
    }

    /// The computer an engine on another machine of the person's belongs to when nothing names
    /// the machine (`other-computer`).
    pub fn other() -> Self {
        Self("other-computer".to_owned())
    }

    /// The name's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ComputerName {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<ComputerName> for String {
    fn from(name: ComputerName) -> String {
        name.0
    }
}

impl fmt::Display for ComputerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which place an id names: the typed view of a [`PlaceId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlaceTarget {
    /// This computer.
    ThisComputer,
    /// One of the person's own computers.
    OwnComputer(ComputerName),
    /// A cloud AI account.
    CloudAccount(AccountId),
}

impl PlaceTarget {
    /// The kind of place this is.
    pub fn kind(&self) -> PlaceKind {
        match self {
            PlaceTarget::ThisComputer => PlaceKind::ThisComputer,
            PlaceTarget::OwnComputer(_) => PlaceKind::OwnComputer,
            PlaceTarget::CloudAccount(_) => PlaceKind::CloudAccount,
        }
    }
}

/// The id of a place: `this-computer`, `computer:<name>` or `account:<account id>`. It can only
/// be made from text that is one of the three, so holding one means it parses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PlaceId(String);

impl PlaceId {
    /// This computer.
    pub fn this_computer() -> Self {
        Self(THIS_COMPUTER.to_owned())
    }

    /// The computer called `name`.
    pub fn computer(name: &ComputerName) -> Self {
        Self(format!("{COMPUTER_PREFIX}{name}"))
    }

    /// The cloud account `account`.
    pub fn account(account: &AccountId) -> Self {
        Self(format!("{ACCOUNT_PREFIX}{account}"))
    }

    /// The id written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let bad = || CoreError::MalformedId {
            what: "place id",
            text: text.to_owned(),
        };
        if text == THIS_COMPUTER {
            return Ok(Self::this_computer());
        }
        if let Some(name) = text.strip_prefix(COMPUTER_PREFIX) {
            return ComputerName::parse(name)
                .map(|name| Self::computer(&name))
                .map_err(|_| bad());
        }
        if let Some(account) = text.strip_prefix(ACCOUNT_PREFIX) {
            return AccountId::parse(account)
                .map(|account| Self::account(&account))
                .map_err(|_| bad());
        }
        Err(bad())
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Which place it names, with the computer's name or the account's id.
    pub fn target(&self) -> PlaceTarget {
        // The text was checked where the id was made; the fallbacks cannot be reached.
        if let Some(name) = self.0.strip_prefix(COMPUTER_PREFIX) {
            return ComputerName::parse(name)
                .map_or(PlaceTarget::ThisComputer, PlaceTarget::OwnComputer);
        }
        if let Some(account) = self.0.strip_prefix(ACCOUNT_PREFIX) {
            return AccountId::parse(account)
                .map_or(PlaceTarget::ThisComputer, PlaceTarget::CloudAccount);
        }
        PlaceTarget::ThisComputer
    }

    /// What kind of place it is.
    pub fn kind(&self) -> PlaceKind {
        self.target().kind()
    }
}

impl TryFrom<String> for PlaceId {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<PlaceId> for String {
    fn from(id: PlaceId) -> String {
        id.0
    }
}

impl fmt::Display for PlaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether a place can answer now. On the bus it is the row's boolean `ready`, data only: it is
/// never put into a sentence a person reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceState {
    /// It can serve now.
    Ready,
    /// It cannot serve now (an engine down, an account to sign in again).
    NotReady,
}

/// One model a place can serve now.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaceModel {
    /// The model's id.
    pub id: porter_core::ModelId,
    /// The name a person reads for it.
    pub name: String,
}

/// One row of `Inference1.Places`: a place known now.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaceRow {
    /// The place's id.
    pub id: PlaceId,
    /// What kind of place it is (the id says it too).
    pub kind: PlaceKind,
    /// The computer's name, or the account's label.
    pub name: String,
    /// The provider's display name; a cloud account only.
    pub provider: Option<String>,
    /// The models the place can serve now, in the order inferd lists them.
    pub models: Vec<PlaceModel>,
    /// Whether the place can serve now.
    pub state: PlaceState,
}

/// Why nothing in the allowed set can serve a request. One reason is named; when several hold,
/// the first of this order: `NotReady`, `FloorRefused`, `ModelNotOffered`, `NoneCapable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoPlaceReason {
    /// An allowed place could serve it, but is not ready now (an engine down, an account to sign
    /// in again, weights not on the computer yet).
    NotReady,
    /// The data class's floor, or the local-only switch, refused every allowed place that could
    /// serve it.
    FloorRefused,
    /// A model pinned for an allowed place (or picked by the person for this kind of work) is not
    /// offered there.
    ModelNotOffered,
    /// No allowed place can do this kind of work at all.
    NoneCapable,
}

impl NoPlaceReason {
    /// Every reason, in the order they win.
    pub const ALL: [NoPlaceReason; 4] = [
        NoPlaceReason::NotReady,
        NoPlaceReason::FloorRefused,
        NoPlaceReason::ModelNotOffered,
        NoPlaceReason::NoneCapable,
    ];

    /// The reason's slug (its serde form).
    pub fn slug(self) -> &'static str {
        match self {
            NoPlaceReason::NotReady => "not_ready",
            NoPlaceReason::FloorRefused => "floor_refused",
            NoPlaceReason::ModelNotOffered => "model_not_offered",
            NoPlaceReason::NoneCapable => "none_capable",
        }
    }

    /// The reason's name in Pascal case, the last segment of its D-Bus error name
    /// (`org.quire.Inference1.Error.NoAllowedPlace.NotReady`).
    pub fn name(self) -> &'static str {
        match self {
            NoPlaceReason::NotReady => "NotReady",
            NoPlaceReason::FloorRefused => "FloorRefused",
            NoPlaceReason::ModelNotOffered => "ModelNotOffered",
            NoPlaceReason::NoneCapable => "NoneCapable",
        }
    }

    /// The reason a Pascal-case name stands for.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.name() == name)
    }
}

/// What inferd answers when a request named the places it may run and none of them can serve it:
/// the reason, and the kind of place outside the set that could have served it, if there is one
/// (so the app can say "turn on a cloud account", not only "it cannot").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaceRefusal {
    /// Why nothing allowed can serve.
    pub reason: NoPlaceReason,
    /// The kind of place that could, outside the allowed set. Absent when no place could.
    pub would_need: Option<PlaceKind>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_form_round_trips_through_its_typed_view() {
        let lab = ComputerName::parse("lab").expect("name");
        let work = AccountId::parse("work-claude").expect("account");
        let cases = [
            ("this-computer", PlaceTarget::ThisComputer, "this_computer"),
            (
                "computer:lab",
                PlaceTarget::OwnComputer(lab.clone()),
                "own_computer",
            ),
            (
                "account:work-claude",
                PlaceTarget::CloudAccount(work.clone()),
                "cloud_account",
            ),
        ];
        for (text, target, slug) in cases {
            let id = PlaceId::parse(text).expect(text);
            assert_eq!(id.as_str(), text);
            assert_eq!(id.to_string(), text);
            assert_eq!(id.target(), target, "{text}");
            assert_eq!(id.kind().slug(), slug, "{text}");
            assert_eq!(PlaceKind::from_slug(slug), Some(id.kind()));
            let json = serde_json::to_string(&id).expect("json");
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(serde_json::from_str::<PlaceId>(&json).expect("back"), id);
        }
        let parsed = |text: &str| PlaceId::parse(text).expect(text);
        assert_eq!(PlaceId::computer(&lab), parsed("computer:lab"));
        assert_eq!(PlaceId::account(&work), parsed("account:work-claude"));
        assert_eq!(PlaceId::this_computer(), parsed("this-computer"));
        assert_eq!(PlaceKind::from_slug("elsewhere"), None);
    }

    #[test]
    fn a_text_that_is_no_place_is_refused() {
        let bad = [
            "",
            "this-computer ",
            "This-computer",
            "this_computer",
            "computer:",
            "computer:Lab",
            "computer:a b",
            "computer:lab server",
            "computer:a/b",
            "account:",
            "account:Work",
            "account:-x",
            "cloud:work",
            "lab",
            "computer:computer:lab",
        ];
        for text in bad {
            assert!(PlaceId::parse(text).is_err(), "{text:?}");
            assert!(
                serde_json::from_value::<PlaceId>(serde_json::Value::String(text.to_owned()))
                    .is_err(),
                "{text:?}"
            );
        }
        let long = format!("computer:{}", "x".repeat(65));
        assert!(PlaceId::parse(&long).is_err());
    }

    #[test]
    fn the_reasons_have_names_that_read_back_and_slugs_that_are_their_serde_form() {
        let mut names = std::collections::BTreeSet::new();
        for reason in NoPlaceReason::ALL {
            assert!(names.insert(reason.name()), "{reason:?}");
            assert_eq!(NoPlaceReason::from_name(reason.name()), Some(reason));
            assert_eq!(
                serde_json::to_string(&reason).expect("json"),
                format!("\"{}\"", reason.slug())
            );
        }
        assert_eq!(NoPlaceReason::from_name("Nope"), None);
        // The order is the order the reasons win in.
        let mut sorted = NoPlaceReason::ALL;
        sorted.sort();
        assert_eq!(sorted, NoPlaceReason::ALL);
    }

    #[test]
    fn open_options_carry_places_and_pins_and_a_default_adds_nothing_to_the_json() {
        use crate::OpenOptions;
        let plain = serde_json::to_string(&OpenOptions::default()).expect("json");
        assert_eq!(plain, r#"{"traceparent":null}"#);
        let options = OpenOptions::default()
            .with_places(vec![
                PlaceId::parse("account:work").expect("id"),
                PlaceId::this_computer(),
            ])
            .with_place_model(
                PlaceId::this_computer(),
                porter_core::ModelId::parse("qwen").expect("id"),
            );
        let json = serde_json::to_string(&options).expect("json");
        assert!(
            json.contains(r#""places":["account:work","this-computer"]"#),
            "{json}"
        );
        assert!(
            json.contains(r#""place_models":{"this-computer":"qwen"}"#),
            "{json}"
        );
        assert_eq!(
            serde_json::from_str::<OpenOptions>(&json).expect("back"),
            options
        );
        // An older client's options read as no places at all, which is not an empty set.
        let old = serde_json::from_str::<OpenOptions>(&plain).expect("old");
        assert_eq!(old.places, None);
        assert_ne!(
            OpenOptions::default().with_places(vec![]).places,
            old.places
        );
    }
}
