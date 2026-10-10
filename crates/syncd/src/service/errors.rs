//! What a method answers instead of a value: a refusal as the error
//! `org.quire.Accounts1.Error.<Refusal>` (`porter_dbus::refusal_error_name` pins the names; one
//! error vocabulary for porter's daemons), or one of the bus's own under its real name, so a
//! client classifies it as it does a bus's refusal. Written by hand, as accountd's: the
//! `DBusError` derive names every non-refusal `org.freedesktop.zbus.Error`.

use porter_core::wire::Refusal;
#[cfg(feature = "photos-picker")]
use porter_dbus::{PICKER_ERROR_NO_SUCH_SESSION, PICKER_ERROR_NOT_YET};
use porter_dbus::{
    SYNC_ERROR_NO_SUCH_CONFLICT, SYNC_ERROR_NOTHING_HELD, SYNC_ERROR_PAUSED, refusal_error_name,
};
use zbus::DBusError;
use zbus::fdo;
use zbus::message::{Header, Message};
use zbus::names::ErrorName;

/// An error reply: its name and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedError {
    name: ErrorName<'static>,
    text: String,
}

impl RefusedError {
    /// The error for `refusal`.
    pub fn of(refusal: Refusal) -> Self {
        let name = ErrorName::try_from(refusal_error_name(refusal)).unwrap_or_else(|_| {
            ErrorName::from_static_str_unchecked("org.freedesktop.DBus.Error.Failed")
        });
        Self {
            name,
            text: String::new(),
        }
    }

    fn bus(error: fdo::Error) -> Self {
        Self {
            name: error.name().into_owned(),
            text: error.description().unwrap_or_default().to_owned(),
        }
    }

    /// An argument that is not what the interface says.
    pub fn invalid(why: impl ToString) -> Self {
        Self::bus(fdo::Error::InvalidArgs(why.to_string()))
    }

    /// A conflict that is not there (unknown, or already settled):
    /// `org.quire.Sync1.Error.NoSuchConflict`.
    pub fn no_such_conflict() -> Self {
        Self {
            name: ErrorName::from_static_str_unchecked(SYNC_ERROR_NO_SUCH_CONFLICT),
            text: "no such conflict, or it is already settled".to_owned(),
        }
    }

    /// `ConfirmDiscard` on a dataset that is not held: `org.quire.Sync1.Error.NothingHeld`.
    pub fn nothing_held() -> Self {
        Self {
            name: ErrorName::from_static_str_unchecked(SYNC_ERROR_NOTHING_HELD),
            text: "nothing is held for confirmation".to_owned(),
        }
    }

    /// `SyncNow` on a dataset the person paused: `org.quire.Sync1.Error.Paused`.
    pub fn paused() -> Self {
        Self {
            name: ErrorName::from_static_str_unchecked(SYNC_ERROR_PAUSED),
            text: "the dataset is paused; resume it first".to_owned(),
        }
    }

    /// `Import` before the person has finished picking: `org.quire.Photos1.Error.NotYet`.
    #[cfg(feature = "photos-picker")]
    pub fn picker_not_yet() -> Self {
        Self {
            name: ErrorName::from_static_str_unchecked(PICKER_ERROR_NOT_YET),
            text: "the person has not finished picking".to_owned(),
        }
    }

    /// A session Google no longer has: `org.quire.Photos1.Error.NoSuchSession`.
    #[cfg(feature = "photos-picker")]
    pub fn picker_no_such_session() -> Self {
        Self {
            name: ErrorName::from_static_str_unchecked(PICKER_ERROR_NO_SUCH_SESSION),
            text: "no such session".to_owned(),
        }
    }

    /// The daemon could not do it.
    pub fn failed(why: impl ToString) -> Self {
        Self::bus(fdo::Error::Failed(why.to_string()))
    }

    /// A caller the daemon will not answer.
    pub fn access_denied(why: impl ToString) -> Self {
        Self::bus(fdo::Error::AccessDenied(why.to_string()))
    }

    /// The error's D-Bus name.
    pub fn error_name(&self) -> &str {
        self.name.as_str()
    }
}

impl DBusError for RefusedError {
    fn create_reply(&self, call: &Header<'_>) -> zbus::Result<Message> {
        Message::error(call, self.name.clone())?.build(&(self.text.as_str(),))
    }

    fn name(&self) -> ErrorName<'_> {
        self.name.clone()
    }

    fn description(&self) -> Option<&str> {
        Some(self.text.as_str())
    }
}
