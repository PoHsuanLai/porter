//! Settling a stored conflict from the bus: the typed words of `Sync1.Resolve`, and the request
//! that goes from the hub to the dataset's driver (which owns the engine and settles it between
//! cycles, so nothing races a cycle writing the journal).

use porter_dbus::{RESOLVE_KEEP_LOCAL, RESOLVE_KEEP_REMOTE};
use porter_sync::Resolution;
use std::str::FromStr;
use tokio::sync::oneshot;

/// A stored conflict's number: the one `Sync1.Conflict` carries under `number`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConflictNumber(pub i64);

/// What `how` said (`keep_local`, `keep_remote`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct How(pub Resolution);

/// A `how` that is neither word.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a way to settle a conflict: use `{RESOLVE_KEEP_LOCAL}` or `{RESOLVE_KEEP_REMOTE}`"
)]
pub struct UnknownHow(pub String);

impl FromStr for How {
    type Err = UnknownHow;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            RESOLVE_KEEP_LOCAL => Ok(How(Resolution::KeepLocal)),
            RESOLVE_KEEP_REMOTE => Ok(How(Resolution::KeepRemote)),
            other => Err(UnknownHow(other.to_owned())),
        }
    }
}

/// Why a conflict was not settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettleError {
    /// There is no such dataset for the caller (never registered, gone, or not the caller's to
    /// see): one answer for all, so a name is never confirmed to a caller who may not have it.
    NoSuchDataset,
    /// The caller sees the dataset but is not its owning app.
    NotOwner,
    /// The conflict is unknown or already settled.
    NoSuchConflict,
    /// The journal or the local side failed.
    Failed(String),
}

/// One request to a dataset's driver.
#[derive(Debug)]
pub struct Settle {
    /// The conflict.
    pub number: ConflictNumber,
    /// The side that wins.
    pub how: How,
    /// Where the driver answers.
    pub reply: oneshot::Sender<Result<(), SettleError>>,
}

/// Why a discard was not let through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmError {
    /// There is no such dataset for the caller (as [`SettleError::NoSuchDataset`]).
    NoSuchDataset,
    /// Nothing is held for confirmation: it never was, or the discard was already confirmed.
    NothingHeld,
}

/// One request to a dataset's driver: let the held discard through.
#[derive(Debug)]
pub struct Confirm {
    /// Where the driver answers.
    pub reply: oneshot::Sender<Result<(), ConfirmError>>,
}

/// Why "Sync now" did not start a cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncNowError {
    /// There is no such dataset (never registered, or gone).
    NoSuchDataset,
    /// The person paused the dataset: it does not cycle until it is resumed.
    Paused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_words_read_and_anything_else_names_them() {
        const CASES: &[(&str, Option<Resolution>)] = &[
            ("keep_local", Some(Resolution::KeepLocal)),
            ("keep_remote", Some(Resolution::KeepRemote)),
            ("", None),
            ("Keep_Local", None),
            ("keep local", None),
            ("local", None),
        ];
        for (text, want) in CASES {
            let got = text.parse::<How>();
            assert_eq!(got.as_ref().ok().map(|how| how.0), *want, "{text:?}");
            if let Err(error) = got {
                let said = error.to_string();
                assert!(said.contains("keep_local") && said.contains("keep_remote"));
            }
        }
    }
}
