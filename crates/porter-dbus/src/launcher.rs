//! The typed errors of the launcher's three `Peer` methods (lane agent-login), beside the
//! refusals of `refusal`: `org.quire.Accounts1.Error.<Name>`. `NoLauncher` is a `Refusal` (the
//! sheet's answer to a login nobody can do); these two are the launcher's own.

use crate::refusal::REFUSAL_ERROR_PREFIX;

/// Why accountd refused a launcher's call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LauncherFault {
    /// A live connection already launches this program (first wins).
    AlreadyRegistered,
    /// No such request for this connection: never made, answered already, expired, or another
    /// connection's.
    UnknownRequest,
}

impl LauncherFault {
    /// Every fault, so the table is total by construction.
    pub const ALL: [LauncherFault; 2] = [
        LauncherFault::AlreadyRegistered,
        LauncherFault::UnknownRequest,
    ];

    /// The error name a daemon replies with.
    pub fn error_name(self) -> String {
        let name = match self {
            LauncherFault::AlreadyRegistered => "AlreadyRegistered",
            LauncherFault::UnknownRequest => "UnknownRequest",
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
