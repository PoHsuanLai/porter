//! What `OpenEndpoint` and `CloseEndpoint` answer instead of a value: a cause as the D-Bus error
//! `org.quire.Inference1.Error.<Cause>`, or one of the bus's own (`AccessDenied`, `InvalidArgs`)
//! under its real name. Written by hand, as accountd's refusals are, so a client tells a cause
//! from any other failure by name.

use porter_dbus::AGENT_ERROR_PREFIX;
use zbus::DBusError;
use zbus::fdo;
use zbus::message::{Header, Message};
use zbus::names::ErrorName;

/// Why an endpoint was not opened (or not closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cause {
    /// The caller is not the agent launcher.
    AccessDenied,
    /// An argument is not what the interface says.
    InvalidArgs,
    /// `ai.agents.endpoint` is off.
    EndpointOff,
    /// None of the program's protocols is one the route serves.
    UnsupportedProtocol,
    /// The route names an account or a model that is not there (or no hosted models at all).
    NoSuchRoute,
    /// The account is not granted to the program for data of this class.
    NotGranted,
    /// The policy keeps data of this class from the route (`ai.local_only`, a floor).
    FloorRefused,
    /// No such session, or not the caller's.
    NoSuchSession,
    /// The daemon could not do it (no randomness, no port).
    Unavailable,
}

impl Cause {
    /// The slug, as the error's name and tests read it.
    pub fn slug(self) -> &'static str {
        match self {
            Cause::AccessDenied => "access_denied",
            Cause::InvalidArgs => "invalid_args",
            Cause::EndpointOff => "endpoint_off",
            Cause::UnsupportedProtocol => "unsupported_protocol",
            Cause::NoSuchRoute => "no_such_route",
            Cause::NotGranted => "not_granted",
            Cause::FloorRefused => "floor_refused",
            Cause::NoSuchSession => "no_such_session",
            Cause::Unavailable => "unavailable",
        }
    }

    fn pascal(self) -> String {
        self.slug()
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The D-Bus error name of this cause.
    pub fn error_name(self) -> String {
        match self {
            Cause::AccessDenied => "org.freedesktop.DBus.Error.AccessDenied".to_owned(),
            Cause::InvalidArgs => "org.freedesktop.DBus.Error.InvalidArgs".to_owned(),
            other => format!("{AGENT_ERROR_PREFIX}{}", other.pascal()),
        }
    }
}

/// An error reply: its cause and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    cause: Cause,
    name: ErrorName<'static>,
    text: String,
}

impl Refusal {
    /// A refusal for `cause` that says `text` (never a key, a token or content).
    pub fn new(cause: Cause, text: impl Into<String>) -> Self {
        let name = ErrorName::try_from(cause.error_name()).unwrap_or_else(|_| {
            ErrorName::from_static_str_unchecked("org.freedesktop.DBus.Error.Failed")
        });
        Self {
            cause,
            name,
            text: text.into(),
        }
    }

    /// The cause.
    pub fn cause(&self) -> Cause {
        self.cause
    }

    /// An argument that is not what the interface says.
    pub fn invalid(text: impl Into<String>) -> Self {
        Self::new(Cause::InvalidArgs, text)
    }

    /// The bus's own error for a failed call into the daemon.
    pub fn from_bus(error: &fdo::Error) -> Self {
        Self::new(Cause::Unavailable, error.to_string())
    }
}

impl DBusError for Refusal {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cause_has_a_distinct_valid_error_name() {
        let all = [
            Cause::AccessDenied,
            Cause::InvalidArgs,
            Cause::EndpointOff,
            Cause::UnsupportedProtocol,
            Cause::NoSuchRoute,
            Cause::NotGranted,
            Cause::FloorRefused,
            Cause::NoSuchSession,
            Cause::Unavailable,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for cause in all {
            let name = cause.error_name();
            assert!(ErrorName::try_from(name.as_str()).is_ok(), "{name}");
            assert!(seen.insert(name), "{cause:?}");
        }
        assert_eq!(
            Cause::EndpointOff.error_name(),
            "org.quire.Inference1.Error.EndpointOff"
        );
        assert_eq!(
            Cause::UnsupportedProtocol.error_name(),
            "org.quire.Inference1.Error.UnsupportedProtocol"
        );
    }
}
