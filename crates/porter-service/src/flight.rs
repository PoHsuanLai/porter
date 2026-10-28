//! One refresh at a time per account, inside a store: the turns (`issue` takes them).
//!
//! An OAuth account's refresh token can rotate: the issuer answers a refresh with a new refresh
//! token and the old one stops working (Microsoft does). Two callers that refresh the same account
//! together would each present the same token and the second would be refused, or would store a
//! token the first has already replaced. So each account has one [`Flight`]: a caller takes its
//! gate, reads the stored credential again (the caller ahead of it may have rotated it), and, if
//! a caller ahead of it refreshed while it waited and issued a token for the same audience, kind
//! and reach that is still good, uses it instead of asking the issuer again. A caller that did
//! not wait behind a refresh always asks the issuer: this is no cache, only the merging of
//! refreshes that ran together. The rotated credential is written back before the new access
//! token is handed out, and before the gate is let go.

use crate::gate::Gate;
use porter_capability::CapabilityKind;
use porter_capability::StorageScope;
use porter_core::AccountId;
use porter_core::{Audience, Credential, IssuedToken};
use porter_ids::UnixSeconds;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// How long before a token expires it is no longer reused: the same margin porter-oauth renews
/// with (`RENEW_MARGIN_SECONDS`), so a caller is never handed a token about to stop working.
const REUSE_MARGIN_SECONDS: i64 = 60;

/// A token a refresh issued.
#[derive(Debug)]
struct Issued {
    audience: Audience,
    /// The grant's kind and reach.
    grant: (CapabilityKind, StorageScope),
    token: IssuedToken,
}

/// What this account's refreshes have issued, and the credential they left in the store.
#[derive(Debug, Default)]
struct Issuing {
    /// The credential the store held after the last refresh made here. Tokens are reused only
    /// while the store still holds exactly this: a sign-in again, or a removal, changes it, and
    /// what was issued under the old one is not handed out.
    left: Option<Credential>,
    issued: Vec<Issued>,
}

/// One account's turn at the issuer.
#[derive(Debug, Default)]
pub(crate) struct Flight {
    pub(crate) gate: Gate,
    issuing: Mutex<Issuing>,
    /// How many refreshes this flight has made: a caller notes it before taking the gate, and
    /// may reuse a token only when a refresh happened while it waited.
    refreshes: AtomicU64,
}

impl Flight {
    fn issuing(&self) -> MutexGuard<'_, Issuing> {
        // Every critical section is a plain data update that cannot panic midway.
        self.issuing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many refreshes have been made so far, noted before taking the gate.
    pub(crate) fn refreshes(&self) -> u64 {
        self.refreshes.load(Ordering::Acquire)
    }

    /// The token the caller ahead of us issued for this audience, kind and reach, when the store
    /// still holds the credential that refresh left and the token is good for longer than the
    /// margin.
    pub(crate) fn reusable(
        &self,
        stored: &Credential,
        audience: &Audience,
        grant: (CapabilityKind, StorageScope),
        now: UnixSeconds,
    ) -> Option<IssuedToken> {
        let issuing = self.issuing();
        if issuing.left.as_ref() != Some(stored) {
            return None;
        }
        issuing
            .issued
            .iter()
            .find(|i| i.audience == *audience && i.grant == grant)
            .filter(|i| i.token.expires.0 - now.0 > REUSE_MARGIN_SECONDS)
            .map(|i| i.token.clone())
    }

    /// Keeps the token just issued by a refresh that presented `presented` and left `left` in
    /// the store (the same credential when the provider did not rotate it). Tokens issued
    /// earlier stay good while the store has held what this flight left; when `presented` is
    /// not that (the store changed outside this flight), they are dropped.
    pub(crate) fn remember(
        &self,
        presented: &Credential,
        left: Credential,
        audience: &Audience,
        grant: (CapabilityKind, StorageScope),
        token: IssuedToken,
    ) {
        let mut issuing = self.issuing();
        self.refreshes.fetch_add(1, Ordering::AcqRel);
        if issuing.left.as_ref() != Some(presented) {
            issuing.issued.clear();
        }
        issuing.left = Some(left);
        issuing
            .issued
            .retain(|i| !(i.audience == *audience && i.grant == grant));
        issuing.issued.push(Issued {
            audience: audience.clone(),
            grant,
            token,
        });
    }
}

/// The flights of a service, one per account that has refreshed.
#[derive(Debug, Default)]
pub(crate) struct Flights {
    accounts: Mutex<HashMap<AccountId, Arc<Flight>>>,
}

impl Flights {
    /// The account's flight.
    pub(crate) fn of(&self, account: &AccountId) -> Arc<Flight> {
        self.accounts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(account.clone())
            .or_default()
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{SecretText, TokenKind};

    fn credential(refresh: &str) -> Credential {
        Credential::OAuth {
            access: SecretText::new("access"),
            refresh: SecretText::new(refresh),
            expires_at: UnixSeconds(10),
            granted: Default::default(),
        }
    }

    fn token(expires: i64) -> IssuedToken {
        IssuedToken::new(
            TokenKind::Bearer,
            SecretText::new("t"),
            UnixSeconds(expires),
        )
    }

    #[test]
    fn a_token_is_reused_for_its_audience_kind_and_reach_while_the_store_holds_what_it_left() {
        let flight = Flight::default();
        let imap = Audience("imap".to_owned());
        let smtp = Audience("smtp".to_owned());
        let graph = Audience("graph".to_owned());
        let mail = (CapabilityKind::Mail, StorageScope::AppFolder);
        let folder = (CapabilityKind::Storage, StorageScope::AppFolder);
        let drive = (CapabilityKind::Storage, StorageScope::Full);
        flight.remember(
            &credential("r1"),
            credential("r2"),
            &imap,
            mail,
            token(1_000),
        );
        flight.remember(
            &credential("r2"),
            credential("r2"),
            &graph,
            folder,
            token(1_000),
        );
        let cases = [
            // (stored refresh, audience, grant, now, reused)
            ("r2", &imap, mail, 100, true),
            ("r2", &graph, folder, 100, true),
            // Another audience, another kind, another reach: asked of the issuer.
            ("r2", &smtp, mail, 100, false),
            (
                "r2",
                &imap,
                (CapabilityKind::Calendar, StorageScope::AppFolder),
                100,
                false,
            ),
            ("r2", &graph, drive, 100, false),
            // The store holds another credential: rotated again, signed in again.
            ("r3", &imap, mail, 100, false),
            // Good for 60 seconds or less is not reused.
            ("r2", &imap, mail, 940, false),
            ("r2", &imap, mail, 939, true),
        ];
        for (refresh, audience, grant, at, want) in cases {
            assert_eq!(
                flight
                    .reusable(&credential(refresh), audience, grant, UnixSeconds(at))
                    .is_some(),
                want,
                "{refresh} {audience:?} {grant:?} {at}"
            );
        }
    }

    #[test]
    fn a_rotation_made_here_keeps_the_tokens_issued_before_it() {
        let flight = Flight::default();
        let imap = Audience("imap".to_owned());
        let smtp = Audience("smtp".to_owned());
        let mail = (CapabilityKind::Mail, StorageScope::AppFolder);
        flight.remember(
            &credential("r1"),
            credential("r2"),
            &imap,
            mail,
            token(1_000),
        );
        // The second refresh presented what the first left, and rotated again.
        flight.remember(
            &credential("r2"),
            credential("r3"),
            &smtp,
            mail,
            token(1_000),
        );
        let at = UnixSeconds(0);
        let reused = |refresh: &str, audience: &Audience| {
            flight
                .reusable(&credential(refresh), audience, mail, at)
                .is_some()
        };
        assert!(reused("r3", &imap) && reused("r3", &smtp));
        // The store no longer holds what the last refresh left.
        assert!(!reused("r2", &imap));
    }

    #[test]
    fn a_change_made_outside_the_flight_drops_what_it_issued() {
        let flight = Flight::default();
        let imap = Audience("imap".to_owned());
        let smtp = Audience("smtp".to_owned());
        let mail = (CapabilityKind::Mail, StorageScope::AppFolder);
        flight.remember(
            &credential("r1"),
            credential("r2"),
            &imap,
            mail,
            token(1_000),
        );
        // The account signed in again (r9), so this refresh did not start from what was left.
        flight.remember(
            &credential("r9"),
            credential("r10"),
            &smtp,
            mail,
            token(1_000),
        );
        let at = UnixSeconds(0);
        let reused = |audience: &Audience| {
            flight
                .reusable(&credential("r10"), audience, mail, at)
                .is_some()
        };
        assert!(reused(&smtp) && !reused(&imap));
    }
}
