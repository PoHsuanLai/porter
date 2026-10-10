//! Signing an agent in and out through its launcher (lane agent-login; design/31 R7-R9, C5, C6).
//! Only the agent can log itself in: porter asks the launcher that registered the agent's
//! program, and the launcher answers with a coarse outcome. Nothing else crosses: no token, no
//! URL, no code, no free text. Every word here is a member of a closed set, so a launcher cannot
//! tunnel the login through the outcome.

use crate::error::CoreError;
use crate::id::is_id;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The id of one request for a login or a logout, made by accountd and sent to one launcher
/// alone. The same grammar as every id of ours.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LoginRequestId(Box<str>);

impl LoginRequestId {
    /// The request id written as `text`.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        match is_id(text) {
            true => Ok(Self(text.into())),
            false => Err(CoreError::MalformedId {
                what: "login request id",
                text: text.to_owned(),
            }),
        }
    }

    /// The id as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LoginRequestId {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<LoginRequestId> for String {
    fn from(id: LoginRequestId) -> String {
        id.0.into()
    }
}

impl fmt::Display for LoginRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why the launcher could not finish a login or a logout: a coarse word from a closed set. It
/// says nothing of how the agent failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoginFault {
    /// The agent or its provider refused the login.
    Refused,
    /// The agent could not reach its provider.
    Unreachable,
    /// The agent program is not installed.
    NotInstalled,
    /// The person did not finish in time, on the launcher's own clock.
    TimedOut,
    /// Anything else.
    Other,
}

impl LoginFault {
    /// Every fault, for tables.
    pub const ALL: [LoginFault; 5] = [
        LoginFault::Refused,
        LoginFault::Unreachable,
        LoginFault::NotInstalled,
        LoginFault::TimedOut,
        LoginFault::Other,
    ];

    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            LoginFault::Refused => "refused",
            LoginFault::Unreachable => "unreachable",
            LoginFault::NotInstalled => "not_installed",
            LoginFault::TimedOut => "timed_out",
            LoginFault::Other => "other",
        }
    }
}

/// What the launcher reports of a login or a logout it was asked for. For a logout `Ready`
/// means done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoginOutcome {
    /// The agent says it is now signed in (or out).
    Ready,
    /// It could not be done, for this coarse reason.
    Failed(LoginFault),
    /// The person or the launcher gave it up.
    Cancelled,
}

impl LoginOutcome {
    /// The two words the bus carries: the outcome and its reason (empty unless `failed`).
    pub fn to_wire(self) -> (&'static str, &'static str) {
        match self {
            LoginOutcome::Ready => ("ready", ""),
            LoginOutcome::Failed(fault) => ("failed", fault.slug()),
            LoginOutcome::Cancelled => ("cancelled", ""),
        }
    }

    /// The outcome two words stand for. A reason with any outcome but `failed`, a `failed` with
    /// no reason, and a word outside the set are all refused.
    pub fn from_wire(outcome: &str, reason: &str) -> Result<Self, CoreError> {
        let fault = LoginFault::ALL.into_iter().find(|f| f.slug() == reason);
        match (outcome, reason, fault) {
            ("ready", "", _) => Ok(LoginOutcome::Ready),
            ("cancelled", "", _) => Ok(LoginOutcome::Cancelled),
            ("failed", _, Some(fault)) => Ok(LoginOutcome::Failed(fault)),
            _ => Err(CoreError::MalformedId {
                what: "login outcome",
                text: format!("{outcome}/{reason}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_OUTCOMES: [LoginOutcome; 7] = [
        LoginOutcome::Ready,
        LoginOutcome::Cancelled,
        LoginOutcome::Failed(LoginFault::Refused),
        LoginOutcome::Failed(LoginFault::Unreachable),
        LoginOutcome::Failed(LoginFault::NotInstalled),
        LoginOutcome::Failed(LoginFault::TimedOut),
        LoginOutcome::Failed(LoginFault::Other),
    ];

    #[test]
    fn every_outcome_reads_back_from_its_words() {
        for outcome in ALL_OUTCOMES {
            let (word, reason) = outcome.to_wire();
            assert_eq!(LoginOutcome::from_wire(word, reason), Ok(outcome));
        }
    }

    #[test]
    fn a_word_outside_the_set_is_refused() {
        for (word, reason) in [
            ("", ""),
            ("ready", "refused"),
            ("cancelled", "other"),
            ("failed", ""),
            ("failed", "https://example.org/device?code=ABCD"),
            ("failed", "Refused"),
            ("done", ""),
            ("ready ", ""),
        ] {
            assert!(
                LoginOutcome::from_wire(word, reason).is_err(),
                "{word}/{reason}"
            );
        }
    }

    #[test]
    fn a_fault_slug_is_its_serde_name() {
        for fault in LoginFault::ALL {
            let json = serde_json::to_value(fault).expect("json");
            assert_eq!(json.as_str(), Some(fault.slug()));
        }
    }

    #[test]
    fn request_ids_are_ids() {
        assert!(LoginRequestId::parse("login-12").is_ok());
        for bad in ["", "Login", "a b", "-x", &"a".repeat(65)] {
            assert!(LoginRequestId::parse(bad).is_err(), "{bad}");
        }
    }
}
