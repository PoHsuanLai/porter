//! The four ways a call to a porter service over the bus fails whatever the call was, held by
//! each service's error ([`AiKeysError`](crate::AiKeysError), [`SpacesError`](crate::SpacesError)
//! and the others) beside the reasons that service gives of its own.

use porter_dbus::{BusError, BusFailure, classify, is_invalid_args};
use porter_ids::IdError;

/// Why a call over the bus failed for a reason that is not the service's own. More reasons may be
/// added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CallFailure {
    /// The service refused the caller: it does not know it, or the call is not one it may make.
    #[error("refused by the service: {0}")]
    Denied(String),
    /// The service refused an argument (an id, a name or a program that is not one).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// The service is not there.
    #[error("no service reachable")]
    Unreachable,
    /// The service sent something that is not porter's protocol, or could not save.
    #[error("malformed: {0}")]
    Malformed(String),
}

impl From<BusError> for CallFailure {
    fn from(error: BusError) -> Self {
        if is_invalid_args(&error) {
            return CallFailure::Invalid(error.to_string());
        }
        match classify(&error) {
            BusFailure::NoDaemon => CallFailure::Unreachable,
            BusFailure::Denied(why) => CallFailure::Denied(why),
            BusFailure::Other(why) => CallFailure::Malformed(why),
            // a failure a later porter-dbus adds
            _ => CallFailure::Malformed("a bus failure this client does not know".into()),
        }
    }
}

impl From<porter_core::CoreError> for CallFailure {
    fn from(error: porter_core::CoreError) -> Self {
        CallFailure::Malformed(error.to_string())
    }
}

impl From<IdError> for CallFailure {
    fn from(error: IdError) -> Self {
        CallFailure::Malformed(error.to_string())
    }
}

/// `impl From<$source> for $error` for each source: it is a [`CallFailure`] held by the error's
/// `Bus` variant. An error whose service gives reasons of its own for some bus errors writes its
/// own `From<BusError>` and leaves `BusError` out of the list.
macro_rules! bus_failure_from {
    ($error:ident: $($source:ty),+) => {
        $(impl From<$source> for $error {
            fn from(source: $source) -> Self {
                $error::Bus(crate::call_failure::CallFailure::from(source))
            }
        })+
    };
}
pub(crate) use bus_failure_from;
