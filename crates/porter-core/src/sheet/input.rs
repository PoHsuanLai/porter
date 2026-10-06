//! What the person did on the sheet, as the host reports it to the daemon (the `Input` signal of
//! `org.quire.AccountsSheet1`). Only the sheet's owner's input counts; the daemon checks the
//! sender, not this value.

use super::fields::FieldAnswer;
use super::progress::ServiceChoice;
use crate::consent::ConsentAnswer;
use crate::id::ProviderId;
use serde::{Deserialize, Serialize};

/// One thing the person did. It may carry secret text (a typed password), the one place
/// secret text flows into the daemon; `Debug` redacts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum SheetInput {
    /// The answer to a consent alert.
    Answer(ConsentAnswer),
    /// A provider picked from the list.
    Pick(ProviderId),
    /// The form submitted, every field at once.
    Submit(Vec<FieldAnswer>),
    /// The review confirmed with these choices ("Add", or "Add, and allow").
    Confirm(Vec<ServiceChoice>),
    /// One step back.
    Back,
    /// Try the failed step again.
    Retry,
    /// "Open Again" on the browser step: open the page once more. The flow does not restart.
    OpenAgain,
    /// The sheet was closed.
    Dismiss,
}
