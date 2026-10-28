//! A bus error as the client's: the one place the library reads what the bus said.

use crate::launcher::LauncherError;
use porter_client::{
    AccountsError, AiAccountsError, AiKeysError, IdentitiesError, LinkError, Malformed, SpacesError,
};
use porter_dbus::accounts::{launcher_fault_of, refusal_of};
use porter_dbus::{
    BusError, BusFailure, classify, is_invalid_args, is_limits_exceeded, is_unknown_object,
};
use porter_wire::Refusal;

/// A failed call: no daemon on the name or no bus is `Unreachable`; the bus's `AccessDenied` (a
/// caller the daemon does not know, or one without the grant) is `Denied`; a refused argument, or an object the daemon does not serve (an account that is
/// not there), is `Invalid`; anything else is the other side not speaking porter's protocol.
pub(crate) fn link_error(error: &BusError) -> LinkError {
    if is_invalid_args(error) || is_unknown_object(error) {
        return LinkError::Invalid;
    }
    match classify(error) {
        BusFailure::NoDaemon => LinkError::Unreachable,
        BusFailure::Denied(_) => LinkError::Denied,
        BusFailure::Other(_) => LinkError::Malformed(Malformed::Reply),
        // a failure a later porter-dbus adds
        _ => LinkError::Malformed(Malformed::UnknownFailure),
    }
}

/// A failed sheet call: the refusal accountd gave, a sheet of this kind open already, or a failure
/// of the bus.
pub(crate) fn accounts_error(error: &BusError) -> AccountsError {
    if let Some(refusal) = refusal_of(error) {
        return AccountsError::Refused(refusal);
    }
    if is_limits_exceeded(error) {
        return AccountsError::Busy;
    }
    AccountsError::Link(link_error(error))
}

/// A failed Identities call: the refusals that mean something to the app, a picker open already,
/// or a failure of the bus.
pub(crate) fn identities_error(error: &BusError) -> IdentitiesError {
    if let Some(refusal) = refusal_of(error) {
        return match refusal {
            Refusal::Dismissed => IdentitiesError::Cancelled,
            Refusal::NoFittingAccount => IdentitiesError::NoIdentity,
            Refusal::NeedsReauth => IdentitiesError::NeedsSignIn,
            Refusal::Unavailable => IdentitiesError::Unavailable,
            other => IdentitiesError::Refused(other),
        };
    }
    if is_limits_exceeded(error) {
        return IdentitiesError::Busy;
    }
    IdentitiesError::Link(link_error(error))
}

/// A failed Spaces call.
pub(crate) fn spaces_error(error: &BusError) -> SpacesError {
    if is_limits_exceeded(error) {
        return SpacesError::TooMany;
    }
    SpacesError::Link(link_error(error))
}

/// A failed call to accountd's Keys.
pub(crate) fn ai_keys_error(error: &BusError) -> AiKeysError {
    match refusal_of(error) {
        Some(refusal) => AiKeysError::Refused(refusal),
        None => AiKeysError::Link(link_error(error)),
    }
}

/// A failed call to accountd's Broker.
pub(crate) fn ai_accounts_error(error: &BusError) -> AiAccountsError {
    match refusal_of(error) {
        Some(refusal) => AiAccountsError::Refused(refusal),
        None => AiAccountsError::Link(link_error(error)),
    }
}

/// A failed call to accountd's Launcher: its own fault, a refusal an app also meets, or a failure
/// of the bus.
pub(crate) fn launcher_error(error: &BusError) -> LauncherError {
    match (launcher_fault_of(error), refusal_of(error)) {
        (Some(fault), _) => LauncherError::Fault(fault),
        (None, Some(refusal)) => LauncherError::Refused(refusal),
        (None, None) => LauncherError::Link(link_error(error)),
    }
}
