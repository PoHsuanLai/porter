//! How a bus call failed, in the terms a client acts on, so a transport maps it without
//! naming zbus's error type.

use zbus::fdo;

/// Why a bus call did not return a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusFailure {
    /// No bus, or nothing owns (or can be activated for) the name: the daemon is not there.
    NoDaemon,
    /// The bus or the daemon refused the caller.
    Denied(String),
    /// Anything else, with the text the bus gave.
    Other(String),
}

/// The error names the bus uses for "the daemon is not there".
const GONE: [&str; 5] = [
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
    "org.freedesktop.DBus.Error.NoServer",
    "org.freedesktop.DBus.Error.Disconnected",
    "org.freedesktop.DBus.Error.Spawn.ServiceNotFound",
];

/// The error names the bus uses for "you may not".
const REFUSED: [&str; 2] = [
    "org.freedesktop.DBus.Error.AccessDenied",
    "org.freedesktop.DBus.Error.AuthFailed",
];

/// The refusal a daemon's error reply stands for (`org.quire.Accounts1.Error.*`), if it is one.
pub fn refusal_of(error: &zbus::Error) -> Option<porter_core::wire::Refusal> {
    let name = match error {
        zbus::Error::MethodError(name, _, _) => name.as_str().to_owned(),
        _ => return None,
    };
    crate::refusal::refusal_from_error_name(&name)
}

/// A bus error as a [`BusFailure`].
pub fn classify(error: &zbus::Error) -> BusFailure {
    let text = error.to_string();
    match error {
        zbus::Error::MethodError(name, _, _) => by_name(name.as_str(), text),
        zbus::Error::FDO(inner) => match inner.as_ref() {
            fdo::Error::ServiceUnknown(_)
            | fdo::Error::NameHasNoOwner(_)
            | fdo::Error::NoServer(_)
            | fdo::Error::Disconnected(_) => BusFailure::NoDaemon,
            fdo::Error::AccessDenied(_) | fdo::Error::AuthFailed(_) => BusFailure::Denied(text),
            _ => BusFailure::Other(text),
        },
        zbus::Error::InputOutput(io) => match io.kind() {
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe => BusFailure::NoDaemon,
            _ => BusFailure::Other(text),
        },
        zbus::Error::Address(_) => BusFailure::NoDaemon,
        _ => BusFailure::Other(text),
    }
}

fn by_name(name: &str, text: String) -> BusFailure {
    match (GONE.contains(&name), REFUSED.contains(&name)) {
        (true, _) => BusFailure::NoDaemon,
        (_, true) => BusFailure::Denied(text),
        _ => BusFailure::Other(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_that_mean_no_daemon_and_refused_are_told_apart() {
        let cases = [
            ("org.freedesktop.DBus.Error.ServiceUnknown", "gone"),
            ("org.freedesktop.DBus.Error.NameHasNoOwner", "gone"),
            ("org.freedesktop.DBus.Error.AccessDenied", "denied"),
            ("org.freedesktop.DBus.Error.NotSupported", "other"),
            ("org.quire.Inference1.Error.Whatever", "other"),
        ];
        for (name, expected) in cases {
            let got = match by_name(name, String::new()) {
                BusFailure::NoDaemon => "gone",
                BusFailure::Denied(_) => "denied",
                BusFailure::Other(_) => "other",
            };
            assert_eq!(got, expected, "{name}");
        }
    }

    #[test]
    fn an_address_error_is_no_daemon() {
        let error = zbus::Error::Address("garbage".to_owned());
        assert_eq!(classify(&error), BusFailure::NoDaemon);
    }
}
