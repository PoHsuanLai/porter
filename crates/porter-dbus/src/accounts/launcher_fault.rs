//! The typed errors of the launcher interface (`org.quire.Accounts1.Launcher`, served by accountd),
//! beside the refusals of `refusal`: `org.quire.Accounts1.Error.<Name>`. The fault values are
//! `porter_wire::LauncherFault`; this reads and writes their bus error names. A reader takes the
//! same names under any service's prefix.

use crate::accounts::ServiceRoot;
use porter_wire::LauncherFault;

/// The error name accountd replies with: the same as [`launcher_fault_error_name_in`] with
/// [`ServiceRoot::Accounts`].
pub fn launcher_fault_error_name(fault: LauncherFault) -> String {
    launcher_fault_error_name_in(fault, ServiceRoot::Accounts)
}

/// The error name the daemon `service` replies with.
pub fn launcher_fault_error_name_in(fault: LauncherFault, service: ServiceRoot) -> String {
    format!("{}.{}", service.error_prefix(), fault.name())
}

/// The fault an error name stands for, if it is one of ours, under any service's prefix.
pub fn launcher_fault_from_error_name(name: &str) -> Option<LauncherFault> {
    LauncherFault::ALL.into_iter().find(|fault| {
        ServiceRoot::ALL
            .into_iter()
            .any(|service| launcher_fault_error_name_in(*fault, service) == name)
    })
}

/// The launcher fault a daemon's error reply stands for, if it is one.
pub fn launcher_fault_of(error: &zbus::Error) -> Option<LauncherFault> {
    match error {
        zbus::Error::MethodError(name, _, _) => launcher_fault_from_error_name(name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fault_has_a_distinct_valid_name_that_reads_back() {
        let mut seen = std::collections::BTreeSet::new();
        for fault in LauncherFault::ALL {
            let name = launcher_fault_error_name(fault);
            assert!(
                zbus::names::ErrorName::try_from(name.as_str()).is_ok(),
                "{name}"
            );
            assert!(seen.insert(name.clone()), "{name} twice");
            assert_eq!(launcher_fault_from_error_name(&name), Some(fault));
            // A launcher's fault is never mistaken for an app-facing refusal.
            assert_eq!(crate::accounts::refusal_from_error_name(&name), None);
        }
        assert_eq!(
            launcher_fault_from_error_name("org.other.UnknownRequest"),
            None
        );
    }
}
