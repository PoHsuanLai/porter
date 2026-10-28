//! accountd's refusals as D-Bus error names: `org.quire.Accounts1.Error.<Slug in Pascal case>`
//! (`NoFittingAccount`, `AudienceNotGranted`), so an app that gets the error knows what to do.

use crate::service_root::ServiceRoot;
use porter_wire::Refusal;

/// The prefix of every accountd refusal's error name (inferd's and syncd's are
/// [`ServiceRoot::error_prefix`] and a dot).
pub const REFUSAL_ERROR_PREFIX: &str = "org.quire.Accounts1.Error.";

/// Every refusal, so the name table is total by construction.
const ALL: [Refusal; 10] = [
    Refusal::Dismissed,
    Refusal::Denied,
    Refusal::NoFittingAccount,
    Refusal::UnknownGrant,
    Refusal::AudienceNotGranted,
    Refusal::NeedsReauth,
    Refusal::Unavailable,
    Refusal::EndpointNotGranted,
    Refusal::NoLauncher,
    Refusal::NeedsConsent,
];

/// The error name accountd replies with for `refusal`. The same as [`refusal_error_name_in`] with
/// [`ServiceRoot::Accounts`].
pub fn refusal_error_name(refusal: Refusal) -> String {
    refusal_error_name_in(ServiceRoot::Accounts, &refusal)
}

/// The error name the daemon `service` replies with for `refusal`: the service's error prefix,
/// then the refusal's slug in Pascal case.
pub fn refusal_error_name_in(service: ServiceRoot, refusal: &Refusal) -> String {
    format!("{}.{}", service.error_prefix(), pascal(&slug_of(*refusal)))
}

/// The refusal an error name stands for, if it is one of ours, from any of the three services
/// (accountd, inferd, syncd).
pub fn refusal_from_error_name(name: &str) -> Option<Refusal> {
    ALL.into_iter().find(|refusal| {
        ServiceRoot::ALL
            .into_iter()
            .any(|service| refusal_error_name_in(service, refusal) == name)
    })
}

fn slug_of(refusal: Refusal) -> String {
    serde_json::to_value(refusal)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn pascal(slug: &str) -> String {
    slug.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_has_a_distinct_valid_error_name_that_reads_back() {
        let mut seen = std::collections::BTreeSet::new();
        for refusal in ALL {
            let name = refusal_error_name(refusal);
            assert!(
                zbus::names::ErrorName::try_from(name.as_str()).is_ok(),
                "{name}"
            );
            assert!(seen.insert(name.clone()), "{name} twice");
            assert_eq!(refusal_from_error_name(&name), Some(refusal));
        }
        assert_eq!(
            refusal_error_name(Refusal::AudienceNotGranted),
            "org.quire.Accounts1.Error.AudienceNotGranted"
        );
        assert_eq!(
            refusal_from_error_name("org.quire.Accounts1.Error.Nope"),
            None
        );
        assert_eq!(refusal_from_error_name("org.other.Denied"), None);
        // Each service names the same refusals under its own prefix, and all three read back.
        for service in ServiceRoot::ALL {
            let name = refusal_error_name_in(service, &Refusal::Denied);
            assert!(name.starts_with(service.error_prefix()), "{name}");
            assert_eq!(refusal_from_error_name(&name), Some(Refusal::Denied));
        }
        assert_eq!(
            refusal_error_name_in(ServiceRoot::Inference, &Refusal::NoLauncher),
            "org.quire.Inference1.Error.NoLauncher"
        );
    }

    #[test]
    fn the_table_lists_every_refusal() {
        // A new Refusal variant must be added to ALL: this match is total, so it will not compile
        // until it is considered here.
        let covered = |refusal: Refusal| match refusal {
            Refusal::Dismissed
            | Refusal::Denied
            | Refusal::NoFittingAccount
            | Refusal::UnknownGrant
            | Refusal::AudienceNotGranted
            | Refusal::NeedsReauth
            | Refusal::Unavailable
            | Refusal::EndpointNotGranted
            | Refusal::NoLauncher
            | Refusal::NeedsConsent => ALL.contains(&refusal),
            // a variant a newer porter adds: not covered, so the test fails until ALL lists it
            _ => false,
        };
        assert!(ALL.into_iter().all(covered));
    }
}
