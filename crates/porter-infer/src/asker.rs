//! Who a session is opened for: the person, or one of their helpers.

use porter_core::HelperId;
use serde::{Deserialize, Serialize};

/// Whose turn a session carries. The person always goes ahead of every helper when the model
/// is busy; helpers wait among themselves in the order they asked.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Asker {
    /// The person's own turn.
    Person,
    /// A job of one of the person's helpers.
    Helper(HelperId),
}

/// Whose Automatic pool a pick reads (`ai.auto.pool.<slot>` for the assistant,
/// `ai.auto.pool.<slot>.helper.<id>` for a helper). A helper never reads the assistant's pool: a
/// helper without a pool of its own has nothing to choose among.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chooser {
    /// The assistant, and the person's own turns.
    #[default]
    Assistant,
    /// One helper.
    Helper(HelperId),
}

impl Asker {
    /// The pool owner this asker chooses with: the person's turn is the assistant's.
    pub fn chooser(&self) -> Chooser {
        match self {
            Asker::Person => Chooser::Assistant,
            Asker::Helper(helper) => Chooser::Helper(helper.clone()),
        }
    }
}

/// The word that starts a helper's text form.
const HELPER_PREFIX: &str = "helper:";

impl Asker {
    /// The text form the bus carries: `person`, or `helper:<id>`.
    pub fn to_text(&self) -> String {
        match self {
            Asker::Person => "person".to_owned(),
            Asker::Helper(helper) => format!("{HELPER_PREFIX}{helper}"),
        }
    }

    /// The asker `text` names (see [`Asker::to_text`]); `None` for anything else.
    pub fn from_text(text: &str) -> Option<Self> {
        match text.strip_prefix(HELPER_PREFIX) {
            Some(id) => HelperId::parse(id).ok().map(Asker::Helper),
            None => (text == "person").then_some(Asker::Person),
        }
    }
}
