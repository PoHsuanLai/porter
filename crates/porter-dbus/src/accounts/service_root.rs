//! The two services that show a sheet to the person and answer with a Request object, and what
//! differs between them: the bus name, the root of the Request objects' paths and the prefix of
//! the errors. The Request interface itself (`org.quire.Accounts1.Request`: `Close`, the signal
//! `Response`) is one shape for all of them.

use crate::sync::SYNC_BUS;
use porter_wire::names::ACCOUNTS_BUS;

/// A daemon that serves Request objects, and so a sheet a caller waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServiceRoot {
    /// accountd (`org.quire.Accounts1`): the sheets for identities, for AI keys and for an
    /// agent's key.
    Accounts,
    /// syncd (`org.quire.Sync1`): its own "Continue as" sheets.
    Sync,
}

impl ServiceRoot {
    /// Every service, so a table over them is total.
    pub const ALL: [ServiceRoot; 2] = [ServiceRoot::Accounts, ServiceRoot::Sync];

    /// The service's well-known bus name.
    pub fn bus_name(self) -> &'static str {
        match self {
            ServiceRoot::Accounts => ACCOUNTS_BUS,
            ServiceRoot::Sync => SYNC_BUS,
        }
    }

    /// The parent of the service's Request objects' paths (`<root>/<sender>/<token>`).
    pub fn request_root(self) -> &'static str {
        match self {
            ServiceRoot::Accounts => "/org/quire/Accounts1/request",
            ServiceRoot::Sync => "/org/quire/Sync1/request",
        }
    }

    /// The prefix of the service's error names, without the dot that follows it
    /// (`org.quire.Accounts1.Error`).
    pub fn error_prefix(self) -> &'static str {
        match self {
            ServiceRoot::Accounts => "org.quire.Accounts1.Error",
            ServiceRoot::Sync => "org.quire.Sync1.Error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_service_has_its_own_valid_name_path_and_error_prefix() {
        let mut names = std::collections::BTreeSet::new();
        for root in ServiceRoot::ALL {
            assert!(
                zbus::names::WellKnownName::try_from(root.bus_name()).is_ok(),
                "{root:?}"
            );
            assert!(
                zbus::zvariant::ObjectPath::try_from(root.request_root()).is_ok(),
                "{root:?}"
            );
            assert!(root.request_root().ends_with("/request"), "{root:?}");
            assert!(names.insert(root.bus_name()), "{root:?}");
            assert!(names.insert(root.request_root()), "{root:?}");
            assert!(names.insert(root.error_prefix()), "{root:?}");
        }
    }
}
