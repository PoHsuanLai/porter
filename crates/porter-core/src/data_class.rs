//! What kind of the user's data a request carries (design/31 §4.5).

use serde::{Deserialize, Serialize};

/// The class of data a request touches or sends. Consent and the AI broker's floors are per
/// class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    /// Data the app itself made (its own library, its own settings).
    AppOwn,
    /// Mail.
    Mail,
    /// Calendars.
    Calendar,
    /// Contacts.
    Contacts,
    /// Notes.
    Notes,
    /// The user's files.
    Files,
    /// Photos.
    Photos,
    /// The clipboard.
    Clipboard,
    /// What is on screen.
    Screen,
    /// Public information (a web page, a question with nothing personal in it).
    Public,
}
