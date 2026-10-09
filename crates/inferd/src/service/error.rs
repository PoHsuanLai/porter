//! What an `Inference1` method answers instead of a value: one of the bus's own errors under its
//! real name, or, for a call that named its `places` and none of them can serve it,
//! `org.quire.Inference1.Error.NoAllowedPlace.<Reason>`.
//!
//! zbus can return an error with a structured body, and this one does: the body of a
//! no-allowed-place reply is two strings, the words for a person and the kind of place outside
//! the set that could have served (`this_computer`, `own_computer`, `cloud_account`; empty when
//! none could). The first string is where every client already looks for an error's text, so a
//! client that knows nothing of the second still reads a sentence; one that does reads
//! `would_need` without parsing prose. Written by hand: the `DBusError` derive cannot carry a
//! second field.

use crate::attached::ComputerError;
use porter_dbus::{COMPUTER_ERROR_PREFIX, PLACE_ERROR_PREFIX};
use porter_infer::{NoPlaceReason, PlaceRefusal};
use zbus::DBusError;
use zbus::fdo;
use zbus::message::{Header, Message};
use zbus::names::ErrorName;

/// An error reply of the `Inference1` object.
#[derive(Debug)]
pub enum InferError {
    /// One of the bus's own errors (`AccessDenied`, `InvalidArgs`, ...).
    Bus(fdo::Error),
    /// None of the places the call named can serve it.
    NoPlace {
        /// The reason and what would have served.
        refusal: PlaceRefusal,
        /// `org.quire.Inference1.Error.NoAllowedPlace.<Reason>`.
        name: ErrorName<'static>,
    },
    /// A computer could not be added or removed: `org.quire.Inference1.Error.Computer.<Name>`
    /// with the sentence Settings shows.
    Computer {
        /// The sentence.
        words: String,
        /// The error's name.
        name: ErrorName<'static>,
    },
}

impl From<fdo::Error> for InferError {
    fn from(error: fdo::Error) -> Self {
        Self::Bus(error)
    }
}

impl From<ComputerError> for InferError {
    fn from(error: ComputerError) -> Self {
        let name = ErrorName::try_from(format!("{COMPUTER_ERROR_PREFIX}{}", error.name()))
            .unwrap_or_else(|_| {
                ErrorName::from_static_str_unchecked("org.freedesktop.DBus.Error.Failed")
            });
        Self::Computer {
            words: error.to_string(),
            name,
        }
    }
}

impl InferError {
    /// The reply for a call whose allowed places cannot serve it.
    pub fn no_place(refusal: PlaceRefusal) -> Self {
        let name = ErrorName::try_from(format!("{PLACE_ERROR_PREFIX}{}", refusal.reason.name()))
            .unwrap_or_else(|_| {
                ErrorName::from_static_str_unchecked("org.freedesktop.DBus.Error.Failed")
            });
        Self::NoPlace { refusal, name }
    }
}

/// What a person is told, in plain words, for each reason.
pub fn words_of(reason: NoPlaceReason) -> &'static str {
    match reason {
        NoPlaceReason::NotReady => "None of the places you allowed is ready to do this right now.",
        NoPlaceReason::FloorRefused => {
            "What this needs to read may not be sent to the places you allowed."
        }
        NoPlaceReason::ModelNotOffered => {
            "The model chosen for one of the places you allowed is not available there."
        }
        NoPlaceReason::NoneCapable => "None of the places you allowed can do this.",
    }
}

impl DBusError for InferError {
    fn create_reply(&self, call: &Header<'_>) -> zbus::Result<Message> {
        match self {
            Self::Bus(error) => error.create_reply(call),
            Self::NoPlace { refusal, name } => Message::error(call, name.clone())?.build(&(
                words_of(refusal.reason),
                refusal.would_need.map_or("", |kind| kind.slug()),
            )),
            Self::Computer { words, name } => {
                Message::error(call, name.clone())?.build(&(words.as_str(),))
            }
        }
    }

    fn name(&self) -> ErrorName<'_> {
        match self {
            Self::Bus(error) => error.name(),
            Self::NoPlace { name, .. } | Self::Computer { name, .. } => name.clone(),
        }
    }

    fn description(&self) -> Option<&str> {
        match self {
            Self::Bus(error) => error.description(),
            Self::NoPlace { refusal, .. } => Some(words_of(refusal.reason)),
            Self::Computer { words, .. } => Some(words),
        }
    }
}
