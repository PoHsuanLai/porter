//! What `find` answers: a closed set the app renders arm by arm.

use porter_core::consent::{Availability, Usage};
use porter_core::{Candidate, DataClass, Need};

/// The answer to "is there an account that can do this for me?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// One granted account fits: use it.
    One(Candidate),
    /// Several granted accounts fit: let the user pick (quire's `AccountPicker`).
    Several(Vec<Candidate>),
    /// Accounts fit but the app holds no grant: ask (`Accounts::request_grant`).
    NeedsConsent(ConsentOffer),
    /// Nothing the app can use.
    None(NoAccount),
}

/// A consent request the app may make, as `find` found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentOffer {
    /// The need.
    pub need: Need,
    /// The data it touches.
    pub class: DataClass,
    /// Interactive or background.
    pub usage: Usage,
}

/// Why there is no account to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoAccount {
    /// None fits; one could be added ("Add Account…").
    NeedsAccount,
    /// The user refused this app every fitting account.
    Denied,
    /// No provider can do this.
    Unsupported,
}

/// The answer from a query's granted candidates and, when there are none, the availability.
pub fn found(candidates: Vec<Candidate>, availability: Availability, offer: ConsentOffer) -> Found {
    match (candidates.len(), availability) {
        (1, _) => Found::One(
            candidates
                .into_iter()
                .next()
                .unwrap_or_else(|| unreachable!("len is 1")),
        ),
        (0, Availability::Granted | Availability::AvailableNeedsConsent) => {
            Found::NeedsConsent(offer)
        }
        (0, Availability::Denied) => Found::None(NoAccount::Denied),
        (0, Availability::NeedsAccount) => Found::None(NoAccount::NeedsAccount),
        (0, Availability::Unsupported) => Found::None(NoAccount::Unsupported),
        _ => Found::Several(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
    use porter_core::need::StorageNeed;
    use porter_core::{AccountLabel, Capability, GrantId, ProviderId, Restriction, Subject};

    fn offer() -> ConsentOffer {
        let need = Need::Storage(StorageNeed {
            access: Access::Read,
            delta: Delta::None,
            scope: StorageScope::AppFolder,
            quota: QuotaReport::Unreported,
        });
        ConsentOffer {
            need,
            class: DataClass::Files,
            usage: Usage::Interactive,
        }
    }

    fn candidate(grant: &str) -> Candidate {
        Candidate {
            account: porter_core::AccountId::parse("cloud").expect("id"),
            label: AccountLabel("Cloud".into()),
            provider: ProviderId::parse("nextcloud").expect("id"),
            subject: Subject::Account,
            capability: Capability::Push(porter_core::capability::PushCap {
                channel: porter_core::capability::PushChannel::LongPoll,
            }),
            restriction: Restriction::none(),
            grant: GrantId::parse(grant).expect("id"),
        }
    }

    #[test]
    fn found_renders_every_state() {
        let cases = vec![
            (
                "one candidate",
                vec![candidate("a")],
                Availability::Granted,
                Found::One(candidate("a")),
            ),
            (
                "two candidates",
                vec![candidate("a"), candidate("b")],
                Availability::Granted,
                Found::Several(vec![candidate("a"), candidate("b")]),
            ),
            (
                "none granted yet",
                vec![],
                Availability::AvailableNeedsConsent,
                Found::NeedsConsent(offer()),
            ),
            (
                "refused",
                vec![],
                Availability::Denied,
                Found::None(NoAccount::Denied),
            ),
            (
                "no account",
                vec![],
                Availability::NeedsAccount,
                Found::None(NoAccount::NeedsAccount),
            ),
            (
                "no provider",
                vec![],
                Availability::Unsupported,
                Found::None(NoAccount::Unsupported),
            ),
        ];
        for (name, candidates, availability, expected) in cases {
            assert_eq!(found(candidates, availability, offer()), expected, "{name}");
        }
    }
}
