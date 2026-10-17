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
