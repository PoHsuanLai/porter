//! The typed errors of the launcher's `Peer` methods (lane agent-login) and of its two `Tokens`
//! methods (lane p2-handoff), beside the refusals of `refusal`: `org.quire.Accounts1.Error.<Name>`.
//! `NoLauncher` is a `Refusal` (the sheet's answer to a login nobody can do); these are the
//! launcher's own.

use crate::refusal::REFUSAL_ERROR_PREFIX;

/// Why accountd refused a launcher's call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LauncherFault {
    /// A live connection already launches this program (first wins).
    AlreadyRegistered,
    /// No such request for this connection: never made, answered already, expired, or another
    /// connection's.
    UnknownRequest,
    /// `IssueProcessCredential` for a program this connection has no live registration for
    /// (`RegisterLauncher`): another connection's, or nobody's.
    NotRegistered,
    /// `IssueProcessCredential` under a `Once` grant: it is spent by its first use, and a second
    /// turn of the agent would find the key gone.
    OnceGrant,
    /// `IssueProcessCredential` under a grant of an account that holds no API key to hand over
    /// (an OAuth account, an agent that signs itself in, a local runtime).
    NotAKeyAccount,
    /// `RevokeProcessCredential` for a credential this connection was not issued: never made,
    /// ended already, or another connection's.
    UnknownCredential,
    /// `BeginSession` for a session id another live connection holds (first wins).
    SessionTaken,
    /// `EndSession`, `RequestAgentGrant` or `IssueProcessCredential` for a session that is not
    /// open for this connection: never begun, ended already, or another connection's.
    UnknownSession,
}

impl LauncherFault {
    /// Every fault, so the table is total by construction.
    pub const ALL: [LauncherFault; 8] = [
        LauncherFault::AlreadyRegistered,
        LauncherFault::UnknownRequest,
        LauncherFault::NotRegistered,
        LauncherFault::OnceGrant,
        LauncherFault::NotAKeyAccount,
        LauncherFault::UnknownCredential,
        LauncherFault::SessionTaken,
        LauncherFault::UnknownSession,
    ];

    /// The error name a daemon replies with.
    pub fn error_name(self) -> String {
        let name = match self {
            LauncherFault::AlreadyRegistered => "AlreadyRegistered",
            LauncherFault::UnknownRequest => "UnknownRequest",
            LauncherFault::NotRegistered => "NotRegistered",
            LauncherFault::OnceGrant => "OnceGrant",
            LauncherFault::NotAKeyAccount => "NotAKeyAccount",
            LauncherFault::UnknownCredential => "UnknownCredential",
            LauncherFault::SessionTaken => "SessionTaken",
            LauncherFault::UnknownSession => "UnknownSession",
        };
        format!("{REFUSAL_ERROR_PREFIX}{name}")
    }

    /// The fault an error name stands for, if it is one of ours.
    pub fn from_error_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|fault| fault.error_name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fault_has_a_distinct_valid_name_that_reads_back() {
        let mut seen = std::collections::BTreeSet::new();
        for fault in LauncherFault::ALL {
            let name = fault.error_name();
            assert!(
                zbus::names::ErrorName::try_from(name.as_str()).is_ok(),
                "{name}"
            );
            assert!(seen.insert(name.clone()), "{name} twice");
            assert_eq!(LauncherFault::from_error_name(&name), Some(fault));
            // A launcher's fault is never mistaken for an app-facing refusal.
            assert_eq!(crate::refusal_from_error_name(&name), None);
        }
        assert_eq!(
            LauncherFault::from_error_name("org.other.UnknownRequest"),
            None
        );
    }
}
