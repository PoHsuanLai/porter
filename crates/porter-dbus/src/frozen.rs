//! The answer of every skeleton method.

use zbus::fdo;

/// The error every skeleton method returns: the interface is frozen, its behaviour not built.
pub(crate) fn error() -> fdo::Error {
    fdo::Error::NotSupported("porter: frozen interface, not implemented".into())
}
